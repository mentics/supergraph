use std::collections::HashMap;

use crate::ast::{ConditionAst, ConditionKind as AstConditionKind, SourceSpan};
use crate::supergraph::{
    self as sg, ConditionKind, Confidence, ContinuationKind, ContinuationPoint, ControlRegion,
    ControlRegionKind, EdgeFact, EdgeKind, Evidence, EvidenceKind, FallthroughBehavior, NodeFact,
    NodeId, NodeKind, ProgramSupergraph, StatementControlEffect, StatementControlEffectKind,
    StatementKind, SyntaxReference, stable_edge_id, stable_id,
};

use crate::id_parts;
use crate::supergraph::ids::Tag;
use super::{
    SemanticCallable, SemanticContext, callable_index::CallableIndex, graph_edge, graph_node,
    insert_edge, insert_node,
    node_owner, span_contains, span_key,
};

const CONTAINMENT_PRECISION: &str = "sg022-structured-region";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let mut index = CallableIndex::build(graph);
    for semantic in context.semantic_callables() {
        emit_callable(graph, &mut index, &semantic);
    }
}

fn emit_callable(
    graph: &mut ProgramSupergraph,
    index: &mut CallableIndex,
    semantic: &SemanticCallable<'_>,
) {
    let mut statements = statements_for_callable(graph, index, semantic);
    if statements.is_empty() {
        return;
    }
    statements.sort_by_key(statement_sort_key);

    let mut condition_ids = Vec::new();
    for condition in semantic
        .conditions()
        .iter()
        .filter(|condition| condition.owner_id == semantic.owner_id())
    {
        if let Some(condition_id) = emit_condition_region(graph, index, semantic, &statements, condition) {
            condition_ids.push(condition_id);
        }
    }

    for statement in statements.iter().filter(|statement| {
        matches!(
            statement.kind,
            StatementKind::Try | StatementKind::Catch | StatementKind::Finally
        )
    }) {
        if let Some(condition_id) = emit_exception_region(graph, semantic, &statements, statement) {
            condition_ids.push(condition_id);
        }
    }

    apply_statement_control_effects(graph, index, semantic.callable().callable_id, &statements);
    index.sync(graph);
    add_condition_contains_edges(graph, index, semantic, &condition_ids);
}

fn emit_condition_region(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    statements: &[StatementInfo],
    condition: &ConditionAst,
) -> Option<NodeId> {
    let controller = controller_for_condition(statements, condition)?;
    let expression_id = expression_id_for_condition(graph, index, semantic, condition.source_span);
    let kind = lowered_condition_kind(condition.kind);
    let regions = match kind {
        ConditionKind::Branch => branch_regions(&controller, statements),
        ConditionKind::Loop => loop_regions(&controller, statements),
        ConditionKind::ConditionalExpression => Vec::new(),
        _ => Vec::new(),
    };
    let controlled_statement_ids = regions
        .iter()
        .flat_map(|region| region.statement_ids.iter().cloned())
        .collect::<Vec<_>>();
    let outcome_labels = if kind == ConditionKind::Loop {
        vec![
            "loop-body".to_string(),
            "loop-exit".to_string(),
            "continue".to_string(),
        ]
    } else {
        let mut labels = vec!["true".to_string(), "false".to_string()];
        if regions
            .iter()
            .any(|region| region.kind == ControlRegionKind::ElseBody)
        {
            labels.push("else".to_string());
        } else {
            labels.push("implicit-fallthrough".to_string());
        }
        labels
    };
    let continuation = continuation_for_controller(&controller, statements, kind);
    let fallthrough = if kind == ConditionKind::Loop {
        FallthroughBehavior::Conditional
    } else if regions
        .iter()
        .any(|region| region.kind == ControlRegionKind::ElseBody)
    {
        FallthroughBehavior::Conditional
    } else {
        FallthroughBehavior::FallsThrough
    };

    let condition_id = stable_id(Tag::Condition,
        id_parts![
            &semantic.callable().callable_id,
            condition_kind_key(kind),
            &span_key(condition.source_span),
            &controller.statement_id,
        ],
    );
    insert_node(
        graph,
        graph_node(
            condition_id,
            NodeKind::Condition,
            node_owner(semantic),
            Some(condition.source_span),
            Confidence::Exact,
            parser_evidence(
                semantic,
                condition.source_span,
                condition_kind_key(kind),
                "structured branch/loop region fact",
            ),
            NodeFact::Condition(sg::Condition {
                condition_id: condition_id,
                callable_id: semantic.callable().callable_id,
                statement_id: Some(controller.statement_id),
                expression_id,
                kind,
                controlled_statement_ids,
                outcome_labels,
                regions,
                continuation,
                fallthrough,
            }),
        ),
    );
    Some(condition_id)
}

fn emit_exception_region(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    statements: &[StatementInfo],
    statement: &StatementInfo,
) -> Option<NodeId> {
    let regions = exception_regions(statement, statements);
    if regions.is_empty() && statement.kind != StatementKind::Try {
        return None;
    }
    let controlled_statement_ids = regions
        .iter()
        .flat_map(|region| region.statement_ids.iter().cloned())
        .collect::<Vec<_>>();
    let condition_id = stable_id(Tag::Condition,
        id_parts![
            &semantic.callable().callable_id,
            "exception-region",
            &statement.statement_id,
            &span_key(statement.span),
        ],
    );
    insert_node(
        graph,
        graph_node(
            condition_id,
            NodeKind::Condition,
            node_owner(semantic),
            Some(statement.span),
            Confidence::Exact,
            parser_evidence(
                semantic,
                statement.span,
                "exception-region",
                "structured try/catch/finally region fact",
            ),
            NodeFact::Condition(sg::Condition {
                condition_id: condition_id,
                callable_id: semantic.callable().callable_id,
                statement_id: Some(statement.statement_id),
                expression_id: None,
                kind: if statement.kind == StatementKind::Catch {
                    ConditionKind::CatchFilter
                } else {
                    ConditionKind::ExceptionRegion
                },
                controlled_statement_ids,
                outcome_labels: vec![
                    "try".to_string(),
                    "exception".to_string(),
                    "finally".to_string(),
                    "normal-continuation".to_string(),
                ],
                regions,
                continuation: continuation_for_exception(statement, statements),
                fallthrough: FallthroughBehavior::Conditional,
            }),
        ),
    );
    Some(condition_id)
}

fn apply_statement_control_effects(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
    statements: &[StatementInfo],
) {
    let mut info_by_id = HashMap::new();
    for info in statements {
        info_by_id.entry(info.statement_id).or_insert(info);
    }
    for &position in index.positions(callable_id) {
        let NodeFact::Statement(statement) = &mut graph.nodes[position].fact else {
            continue;
        };
        let Some(info) = info_by_id.get(&statement.statement_id).copied() else {
            continue;
        };
        let next = next_statement_id(info, statements);
        statement.control_effects = control_effects_for_statement(info, next);
    }
}

fn control_effects_for_statement(
    statement: &StatementInfo,
    next_statement_id: Option<NodeId>,
) -> Vec<StatementControlEffect> {
    let (kind, fallthrough, description) = match statement.kind {
        StatementKind::Break => (
            StatementControlEffectKind::Break,
            FallthroughBehavior::DoesNotFallThrough,
            "break transfers to the nearest loop or switch exit",
        ),
        StatementKind::Continue => (
            StatementControlEffectKind::Continue,
            FallthroughBehavior::DoesNotFallThrough,
            "continue transfers to the nearest loop continuation point",
        ),
        StatementKind::Return => (
            StatementControlEffectKind::Return,
            FallthroughBehavior::DoesNotFallThrough,
            "return transfers to callable normal exit",
        ),
        StatementKind::Raise => (
            StatementControlEffectKind::Raise,
            FallthroughBehavior::DoesNotFallThrough,
            "raise transfers to the nearest handler or callable exceptional exit",
        ),
        StatementKind::Throw => (
            StatementControlEffectKind::Throw,
            FallthroughBehavior::DoesNotFallThrough,
            "throw transfers to the nearest handler or callable exceptional exit",
        ),
        StatementKind::Try => (
            StatementControlEffectKind::TryEnter,
            FallthroughBehavior::Conditional,
            "try enters protected statements with exceptional handler/finally paths",
        ),
        StatementKind::Catch => (
            StatementControlEffectKind::CatchEnter,
            FallthroughBehavior::Conditional,
            "catch handles matching exceptional paths",
        ),
        StatementKind::Finally => (
            StatementControlEffectKind::FinallyEnter,
            FallthroughBehavior::Conditional,
            "finally runs after normal or exceptional try/catch paths",
        ),
        _ => (
            StatementControlEffectKind::NormalFallthrough,
            FallthroughBehavior::FallsThrough,
            "statement normally falls through to the next statement",
        ),
    };

    vec![StatementControlEffect {
        kind,
        target_statement_id: next_statement_id,
        fallthrough,
        description: description.to_string(),
    }]
}

fn add_condition_contains_edges(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    condition_ids: &[NodeId],
) {
    for condition_id in condition_ids {
        let Some(condition) = condition_fact(graph, index, *condition_id).cloned() else {
            continue;
        };
        for region in &condition.regions {
            for member_id in &region.statement_ids {
                insert_edge(
                    graph,
                    graph_edge(
                        stable_edge_id(id_parts!["contains", condition_id, member_id, CONTAINMENT_PRECISION],
                        ),
                        EdgeKind::Contains,
                        *condition_id,
                        *member_id,
                        node_owner(semantic),
                        None,
                        Confidence::Exact,
                        parser_evidence(
                            semantic,
                            index
                                .node_position(*member_id)
                                .and_then(|position| graph.nodes[position].span)
                                .unwrap_or_default(),
                            "structured-region",
                            "structured control region containment",
                        ),
                        EdgeFact::Contains(sg::Contains {
                            container_id: *condition_id,
                            member_id: *member_id,
                        }),
                    ),
                );
            }
        }
    }
}

fn statements_for_callable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
) -> Vec<StatementInfo> {
    let mut text_by_span = HashMap::new();
    for ast in semantic.statements() {
        text_by_span.entry(ast.source_span).or_insert(&ast.text);
    }
    index
        .nodes(graph, semantic.callable().callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Statement(statement) => Some(StatementInfo {
                statement_id: statement.statement_id,
                parent_statement_id: statement.parent_statement_id,
                kind: statement.kind,
                ordinal: statement.ordinal,
                text: text_by_span
                    .get(&node.span.unwrap_or_default())
                    .map(|text| (*text).clone())
                    .unwrap_or_default(),
                span: node.span?,
            }),
            _ => None,
        })
        .collect()
}

fn controller_for_condition(
    statements: &[StatementInfo],
    condition: &ConditionAst,
) -> Option<StatementInfo> {
    statements
        .iter()
        .filter(|statement| {
            matches!(statement.kind, StatementKind::Branch | StatementKind::Loop)
                && span_contains(statement.span, condition.source_span)
        })
        .min_by_key(|statement| {
            statement
                .span
                .end_byte
                .saturating_sub(statement.span.start_byte)
        })
        .cloned()
}

fn expression_id_for_condition(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    condition_span: SourceSpan,
) -> Option<NodeId> {
    index
        .nodes(graph, semantic.callable().callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Expression(expression) => Some((node, expression)),
            _ => None,
        })
        .filter(|(node, _)| {
            node.span == Some(condition_span)
                || node
                    .span
                    .is_some_and(|span| span_contains(condition_span, span))
        })
        .min_by_key(|(node, _)| {
            node.span
                .map(|span| span.end_byte.saturating_sub(span.start_byte))
                .unwrap_or(usize::MAX)
        })
        .map(|(_, expression)| expression.expression_id)
}

fn branch_regions(statement: &StatementInfo, statements: &[StatementInfo]) -> Vec<ControlRegion> {
    let else_offset = marker_offset(
        &statement.text,
        statement.span.start_byte,
        &["else", "elif"],
    );
    let descendants = descendants(statement, statements)
        .into_iter()
        .filter(|child| child.statement_id != statement.statement_id)
        .collect::<Vec<_>>();
    let branch_ids = descendants
        .iter()
        .filter(|child| else_offset.is_none_or(|offset| child.span.start_byte < offset))
        .map(|child| child.statement_id)
        .collect::<Vec<_>>();
    let else_ids = descendants
        .iter()
        .filter(|child| else_offset.is_some_and(|offset| child.span.start_byte >= offset))
        .map(|child| child.statement_id)
        .collect::<Vec<_>>();

    let mut regions = vec![region(
        ControlRegionKind::BranchBody,
        "branch-body",
        branch_ids,
        FallthroughBehavior::Conditional,
    )];
    if else_offset.is_some() {
        regions.push(region(
            ControlRegionKind::ElseBody,
            "else-body",
            else_ids,
            FallthroughBehavior::Conditional,
        ));
    }
    regions
}

fn loop_regions(statement: &StatementInfo, statements: &[StatementInfo]) -> Vec<ControlRegion> {
    let body_ids = descendants(statement, statements)
        .into_iter()
        .filter(|child| child.statement_id != statement.statement_id)
        .map(|child| child.statement_id)
        .collect::<Vec<_>>();
    vec![
        region(
            ControlRegionKind::LoopBody,
            "loop-body",
            body_ids,
            FallthroughBehavior::Conditional,
        ),
        region(
            ControlRegionKind::LoopContinuation,
            "loop-continuation",
            vec![statement.statement_id],
            FallthroughBehavior::Conditional,
        ),
    ]
}

fn exception_regions(
    statement: &StatementInfo,
    statements: &[StatementInfo],
) -> Vec<ControlRegion> {
    let catch_offset = marker_offset(
        &statement.text,
        statement.span.start_byte,
        &["except", "catch"],
    );
    let finally_offset = marker_offset(&statement.text, statement.span.start_byte, &["finally"]);
    let descendants = descendants(statement, statements)
        .into_iter()
        .filter(|child| child.statement_id != statement.statement_id)
        .collect::<Vec<_>>();

    let try_ids = descendants
        .iter()
        .filter(|child| {
            catch_offset.is_none_or(|offset| child.span.start_byte < offset)
                && finally_offset.is_none_or(|offset| child.span.start_byte < offset)
        })
        .map(|child| child.statement_id)
        .collect::<Vec<_>>();
    let catch_ids = descendants
        .iter()
        .filter(|child| {
            catch_offset.is_some_and(|offset| child.span.start_byte >= offset)
                && finally_offset.is_none_or(|offset| child.span.start_byte < offset)
        })
        .map(|child| child.statement_id)
        .collect::<Vec<_>>();
    let finally_ids = descendants
        .iter()
        .filter(|child| finally_offset.is_some_and(|offset| child.span.start_byte >= offset))
        .map(|child| child.statement_id)
        .collect::<Vec<_>>();

    let mut regions = vec![region(
        ControlRegionKind::TryBody,
        "try-body",
        try_ids,
        FallthroughBehavior::Conditional,
    )];
    if catch_offset.is_some() {
        regions.push(region(
            ControlRegionKind::CatchBody,
            "catch-body",
            catch_ids,
            FallthroughBehavior::Conditional,
        ));
    }
    if finally_offset.is_some() {
        regions.push(region(
            ControlRegionKind::FinallyBody,
            "finally-body",
            finally_ids,
            FallthroughBehavior::Conditional,
        ));
    }
    regions
}

fn region(
    kind: ControlRegionKind,
    label: &str,
    statement_ids: Vec<NodeId>,
    fallthrough: FallthroughBehavior,
) -> ControlRegion {
    ControlRegion {
        kind,
        label: label.to_string(),
        entry_statement_id: statement_ids.first().cloned(),
        exit_statement_id: statement_ids.last().cloned(),
        statement_ids,
        fallthrough,
    }
}

fn descendants<'a>(
    statement: &StatementInfo,
    statements: &'a [StatementInfo],
) -> Vec<&'a StatementInfo> {
    statements
        .iter()
        .filter(|candidate| {
            candidate.statement_id != statement.statement_id
                && span_contains(statement.span, candidate.span)
        })
        .collect()
}

fn continuation_for_controller(
    statement: &StatementInfo,
    statements: &[StatementInfo],
    kind: ConditionKind,
) -> Option<ContinuationPoint> {
    let next = next_statement_id(statement, statements);
    let (continuation_kind, target, description) = if kind == ConditionKind::Loop {
        (
            ContinuationKind::LoopCondition,
            Some(statement.statement_id),
            "loop body continuation returns to the loop condition",
        )
    } else {
        (
            ContinuationKind::NextStatement,
            next,
            "branch continuation falls through to the next sibling statement",
        )
    };
    Some(ContinuationPoint {
        kind: continuation_kind,
        target_statement_id: target,
        description: description.to_string(),
    })
}

fn continuation_for_exception(
    statement: &StatementInfo,
    statements: &[StatementInfo],
) -> Option<ContinuationPoint> {
    let regions = exception_regions(statement, statements);
    let finally_entry = regions
        .iter()
        .find(|region| region.kind == ControlRegionKind::FinallyBody)
        .and_then(|region| region.entry_statement_id);
    Some(ContinuationPoint {
        kind: if finally_entry.is_some() {
            ContinuationKind::Finally
        } else {
            ContinuationKind::ExceptionHandler
        },
        target_statement_id: finally_entry.or_else(|| next_statement_id(statement, statements)),
        description: "exception region continues through matching handlers and finally paths"
            .to_string(),
    })
}

fn next_statement_id(statement: &StatementInfo, statements: &[StatementInfo]) -> Option<NodeId> {
    statements
        .iter()
        .filter(|candidate| candidate.parent_statement_id == statement.parent_statement_id)
        .filter(|candidate| candidate.span.start_byte > statement.span.start_byte)
        .min_by_key(|candidate| candidate.span.start_byte)
        .map(|candidate| candidate.statement_id)
}

fn marker_offset(text: &str, start_byte: usize, markers: &[&str]) -> Option<usize> {
    markers
        .iter()
        .filter_map(|marker| text.find(marker).map(|offset| start_byte + offset))
        .min()
}

fn condition_fact<'a>(
    graph: &'a ProgramSupergraph,
    index: &CallableIndex,
    condition_id: NodeId,
) -> Option<&'a sg::Condition> {
    // Condition ids are derived from their node id, so the first node with this id is the fact.
    index
        .node_position(condition_id)
        .and_then(|position| match &graph.nodes[position].fact {
            NodeFact::Condition(condition) if condition.condition_id == condition_id => {
                Some(condition)
            }
            _ => None,
        })
}

fn lowered_condition_kind(kind: AstConditionKind) -> ConditionKind {
    match kind {
        AstConditionKind::If | AstConditionKind::ElseIf => ConditionKind::Branch,
        AstConditionKind::While | AstConditionKind::For => ConditionKind::Loop,
        AstConditionKind::ConditionalExpression => ConditionKind::ConditionalExpression,
    }
}

fn condition_kind_key(kind: ConditionKind) -> &'static str {
    match kind {
        ConditionKind::Branch => "branch",
        ConditionKind::Loop => "loop",
        ConditionKind::Guard => "guard",
        ConditionKind::MatchArm => "match-arm",
        ConditionKind::CatchFilter => "catch-filter",
        ConditionKind::ExceptionRegion => "exception-region",
        ConditionKind::ShortCircuit => "short-circuit",
        ConditionKind::ConditionalExpression => "conditional-expression",
        ConditionKind::Unknown => "unknown",
    }
}

fn parser_evidence(
    semantic: &SemanticCallable<'_>,
    source_span: SourceSpan,
    syntax_kind: &str,
    summary: &str,
) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Parser,
        summary: summary.to_string(),
        source_id: Some(semantic.artifact().artifact_id),
        source_span: Some(source_span),
        content_hash: semantic.artifact().content_hash.clone(),
        syntax: Some(SyntaxReference {
            kind: syntax_kind.to_string(),
            node_key: Some(format!("structured:{}", span_key(source_span))),
            field_path: Vec::new(),
        }),
    }]
}

fn statement_sort_key(statement: &StatementInfo) -> (usize, usize, StatementKind, usize, NodeId) {
    (
        statement.span.start_byte,
        statement.span.end_byte,
        statement.kind,
        statement.ordinal,
        statement.statement_id,
    )
}

#[derive(Debug, Clone)]
struct StatementInfo {
    statement_id: NodeId,
    parent_statement_id: Option<NodeId>,
    kind: StatementKind,
    ordinal: usize,
    text: String,
    span: SourceSpan,
}

#[cfg(test)]
mod tests {
    use crate::supergraph::ids::NodeId;
    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, ConditionAst, ConditionKind as AstConditionKind, ExpressionAst,
        ExpressionKind as AstExpressionKind, FileAst, ProjectAst, SourceSpan, StatementAst,
        StatementKind as AstStatementKind,
    };
    use crate::supergraph::{
        Condition, ConditionKind, ContinuationKind, ControlRegionKind, FallthroughBehavior,
        NodeFact, ProgramSupergraph, Statement, StatementControlEffectKind, StatementKind,
    };

    #[test]
    fn sg022_lowers_python_structured_regions_and_exception_effects() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));

        assert_branch_regions(&graph, true);
        assert_loop_regions(&graph);
        assert_exception_regions(&graph);
        assert_statement_effect(
            &graph,
            StatementKind::Raise,
            StatementControlEffectKind::Raise,
        );
        assert_statement_effect(
            &graph,
            StatementKind::Continue,
            StatementControlEffectKind::Continue,
        );
    }

    #[test]
    fn sg022_lowers_typescript_fallthrough_and_throw_structure() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));

        assert_branch_regions(&graph, false);
        assert_loop_regions(&graph);
        assert_exception_regions(&graph);
        assert_statement_effect(
            &graph,
            StatementKind::Throw,
            StatementControlEffectKind::Throw,
        );
        assert_statement_effect(
            &graph,
            StatementKind::Break,
            StatementControlEffectKind::Break,
        );
    }

    fn assert_branch_regions(graph: &ProgramSupergraph, has_else: bool) {
        let branch = condition(graph, ConditionKind::Branch, span(3, 7));
        assert_eq!(branch.expression_id, Some(expression_id(graph, span(3, 7))));
        assert_eq!(
            branch.fallthrough,
            if has_else {
                FallthroughBehavior::Conditional
            } else {
                FallthroughBehavior::FallsThrough
            }
        );
        assert_eq!(
            branch.continuation.as_ref().map(|point| point.kind),
            Some(ContinuationKind::NextStatement)
        );
        assert!(
            branch
                .regions
                .iter()
                .any(|region| region.kind == ControlRegionKind::BranchBody)
        );
        if has_else {
            assert!(
                branch
                    .regions
                    .iter()
                    .any(|region| region.kind == ControlRegionKind::ElseBody)
            );
            assert!(branch.outcome_labels.contains(&"else".to_string()));
        } else {
            assert!(
                branch
                    .outcome_labels
                    .contains(&"implicit-fallthrough".to_string())
            );
        }
        assert!(!branch.controlled_statement_ids.is_empty());
        assert_region_contains_edges(graph, branch);
    }

    fn assert_loop_regions(graph: &ProgramSupergraph) {
        let loop_condition = condition(graph, ConditionKind::Loop, span(76, 81));
        assert_eq!(
            loop_condition.continuation.as_ref().map(|point| point.kind),
            Some(ContinuationKind::LoopCondition)
        );
        assert!(
            loop_condition
                .regions
                .iter()
                .any(|region| region.kind == ControlRegionKind::LoopBody)
        );
        assert!(
            loop_condition
                .regions
                .iter()
                .any(|region| region.kind == ControlRegionKind::LoopContinuation)
        );
        assert!(
            loop_condition
                .outcome_labels
                .contains(&"continue".to_string())
        );
        assert_region_contains_edges(graph, loop_condition);
    }

    fn assert_exception_regions(graph: &ProgramSupergraph) {
        let exception = condition(graph, ConditionKind::ExceptionRegion, span(130, 240));
        for kind in [
            ControlRegionKind::TryBody,
            ControlRegionKind::CatchBody,
            ControlRegionKind::FinallyBody,
        ] {
            assert!(
                exception.regions.iter().any(|region| region.kind == kind),
                "missing exception region {kind:?}"
            );
        }
        assert_eq!(
            exception.continuation.as_ref().map(|point| point.kind),
            Some(ContinuationKind::Finally)
        );
        assert!(exception.outcome_labels.contains(&"exception".to_string()));
        assert_region_contains_edges(graph, exception);
    }

    fn assert_statement_effect(
        graph: &ProgramSupergraph,
        statement_kind: StatementKind,
        effect_kind: StatementControlEffectKind,
    ) {
        let statement = statements(graph)
            .into_iter()
            .find(|statement| statement.kind == statement_kind)
            .unwrap_or_else(|| panic!("missing statement kind {statement_kind:?}"));
        assert!(
            statement
                .control_effects
                .iter()
                .any(|effect| effect.kind == effect_kind
                    && effect.fallthrough == FallthroughBehavior::DoesNotFallThrough)
        );
    }

    fn assert_region_contains_edges(graph: &ProgramSupergraph, condition: &Condition) {
        for member_id in condition
            .regions
            .iter()
            .flat_map(|region| region.statement_ids.iter())
        {
            assert!(
                graph.edges.iter().any(|edge| {
                    edge.source_id == condition.condition_id
                        && edge.target_id == Some(*member_id)
                }),
                "missing structured containment edge to {member_id}"
            );
        }
    }

    fn condition(graph: &ProgramSupergraph, kind: ConditionKind, span: SourceSpan) -> &Condition {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Condition(condition)
                    if condition.kind == kind && node.span == Some(span) =>
                {
                    Some(condition)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing condition {kind:?} at {span:?}"))
    }

    fn statements(graph: &ProgramSupergraph) -> Vec<&Statement> {
        graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Statement(statement) => Some(statement),
                _ => None,
            })
            .collect()
    }

    fn expression_id(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Expression(expression) if node.span == Some(span) => {
                    Some(expression.expression_id)
                }
                _ => None,
            })
            .expect("condition expression")
    }

    fn project(path: &str, owner_id: &str, python: bool) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: Vec::new(),
                statements: statements_fixture(owner_id, python),
                expressions: expressions_fixture(owner_id),
                conditions: conditions_fixture(owner_id),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
                parse_errors: Vec::new(),
            }],
        }
    }

    fn statements_fixture(owner_id: &str, python: bool) -> Vec<StatementAst> {
        let branch_text = if python {
            "if flag:\n    true_body()\nelse:\n    false_body()"
        } else {
            "if (flag) {\n  trueBody();\n}"
        };
        let try_text = if python {
            "try:\n    raise Error()\nexcept Error:\n    recover()\nfinally:\n    cleanup()"
        } else {
            "try {\n  throw error;\n} catch (error) {\n  recover();\n} finally {\n  cleanup();\n}"
        };
        vec![
            statement(AstStatementKind::If, branch_text, owner_id, span(0, 60)),
            statement(
                AstStatementKind::Expression,
                "true_body()",
                owner_id,
                span(10, 20),
            ),
            if python {
                statement(
                    AstStatementKind::Expression,
                    "false_body()",
                    owner_id,
                    span(35, 45),
                )
            } else {
                statement(
                    AstStatementKind::Expression,
                    "after_if()",
                    owner_id,
                    span(62, 68),
                )
            },
            statement(
                AstStatementKind::Loop,
                "while items:\n    tick()",
                owner_id,
                span(70, 120),
            ),
            statement(
                AstStatementKind::Expression,
                "tick()",
                owner_id,
                span(84, 89),
            ),
            statement(
                AstStatementKind::Continue,
                "continue",
                owner_id,
                span(92, 100),
            ),
            statement(AstStatementKind::Break, "break", owner_id, span(104, 109)),
            statement(AstStatementKind::Try, try_text, owner_id, span(130, 240)),
            if python {
                statement(
                    AstStatementKind::Raise,
                    "raise Error()",
                    owner_id,
                    span(142, 155),
                )
            } else {
                statement(
                    AstStatementKind::Throw,
                    "throw error",
                    owner_id,
                    span(142, 155),
                )
            },
            statement(AstStatementKind::Catch, "catch", owner_id, span(172, 200)),
            statement(
                AstStatementKind::Expression,
                "recover()",
                owner_id,
                span(178, 187),
            ),
            statement(
                AstStatementKind::Finally,
                "finally",
                owner_id,
                span(208, 234),
            ),
            statement(
                AstStatementKind::Expression,
                "cleanup()",
                owner_id,
                span(216, 225),
            ),
        ]
    }

    fn expressions_fixture(owner_id: &str) -> Vec<ExpressionAst> {
        vec![
            expression(AstExpressionKind::Identifier, "flag", owner_id, span(3, 7)),
            expression(
                AstExpressionKind::Identifier,
                "items",
                owner_id,
                span(76, 81),
            ),
        ]
    }

    fn conditions_fixture(owner_id: &str) -> Vec<ConditionAst> {
        vec![
            ConditionAst {
                kind: AstConditionKind::If,
                text: "flag".to_string(),
                owner_id: owner_id.to_string(),
                source_span: span(3, 7),
            },
            ConditionAst {
                kind: AstConditionKind::While,
                text: "items".to_string(),
                owner_id: owner_id.to_string(),
                source_span: span(76, 81),
            },
        ]
    }

    fn statement(
        kind: AstStatementKind,
        text: &str,
        owner_id: &str,
        source_span: SourceSpan,
    ) -> StatementAst {
        StatementAst {
            kind,
            text: text.to_string(),
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn expression(
        kind: AstExpressionKind,
        text: &str,
        owner_id: &str,
        source_span: SourceSpan,
    ) -> ExpressionAst {
        ExpressionAst {
            kind,
            text: text.to_string(),
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn span(start_byte: usize, end_byte: usize) -> SourceSpan {
        SourceSpan {
            start_byte,
            end_byte,
            start_row: start_byte,
            start_column: 0,
            end_row: end_byte,
            end_column: 0,
        }
    }
}
