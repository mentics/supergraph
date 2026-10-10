use crate::intern::Sym;
use std::collections::{BTreeMap, HashMap};

use crate::ast::SourceSpan;
use crate::supergraph::ids::Tag;
use crate::supergraph::{
    self as sg, Confidence, EdgeFact, EdgeKind, Evidence, EvidenceKind, NodeFact, NodeId, NodeKind,
    ProgramSupergraph, ScopeBindingBehavior, ScopeKind, ScopeVariant, SourceOwnership,
    SyntaxReference, stable_id,
};

use super::{
    SemanticContext, edge_id, graph_edge, graph_node, insert_edge, span_contains,
    span_key,
};

const CONTAINMENT_PRECISION: &str = "sg024-lexical-scope-containment";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    normalize_existing_scopes(graph);
    emit_region_scopes(graph);
    emit_declaration_scopes(graph, context);
    emit_comprehension_scopes(graph);
    retarget_owned_facts_to_lexical_scopes(graph);
    emit_scope_member_edges(graph);
}

#[derive(Debug, Clone)]
struct ScopeInfo {
    scope_id: NodeId,
    kind: ScopeKind,
    span: Option<SourceSpan>,
    binding_behavior: ScopeBindingBehavior,
}

#[derive(Debug, Clone)]
struct StatementInfo {
    span: SourceSpan,
}

fn normalize_existing_scopes(graph: &mut ProgramSupergraph) {
    let language = graph.language.clone();
    for node in &mut graph.nodes {
        let NodeFact::Scope(scope) = &mut node.fact else {
            continue;
        };
        scope.variant = variant_for_existing_scope(scope.kind);
        scope.language_variant = (language_variant(scope.kind, scope.variant, Some(&language))).map(Sym::from);
        scope.binding_behavior = ScopeBindingBehavior::Boundary;
    }
}

fn emit_region_scopes(graph: &mut ProgramSupergraph) {
    let language_by_artifact = language_by_artifact(graph);
    let callable_by_id = callable_artifacts(graph);
    let statement_spans = statement_spans(graph);
    let mut planned = Vec::new();
    // Regions of one construct can cover the same span (a loop body and its continuation).
    // Two scopes with an identical span would make "innermost scope" depend on id order and
    // split bindings from their uses, so only the first region per span gets a scope.
    let mut seen_region_spans = std::collections::BTreeSet::new();

    for node in &graph.nodes {
        let NodeFact::Condition(condition) = &node.fact else {
            continue;
        };
        let Some((artifact_id, language)) =
            callable_by_id
                .get(&condition.callable_id)
                .and_then(|artifact_id| {
                    language_by_artifact
                        .get(artifact_id)
                        .map(|language| (artifact_id, language))
                })
        else {
            continue;
        };
        for region in &condition.regions {
            let Some(region_span) = span_for_statement_ids(&region.statement_ids, &statement_spans)
            else {
                continue;
            };
            if !seen_region_spans.insert((*artifact_id, condition.callable_id, region.kind == sg::ControlRegionKind::CatchBody, region_span)) {
                continue;
            }
            let (kind, variant) = if region.kind == sg::ControlRegionKind::CatchBody {
                (ScopeKind::Catch, ScopeVariant::CatchHandler)
            } else {
                (ScopeKind::Block, ScopeVariant::StatementBlock)
            };
            planned.push(PlannedScope {
                scope_id: stable_id(
                    Tag::Scope,
                    crate::id_parts![
                        artifact_id,
                        "region",
                        condition.callable_id,
                        node.node_id,
                        &region.label,
                        &span_key(region_span),
                    ],
                ),
                kind,
                variant,
                language: language.clone(),
                artifact_id: artifact_id.clone(),
                owner_callable_id: Some(condition.callable_id.clone()),
                span: Some(region_span),
            });
        }
    }

    insert_planned_scopes(graph, planned);
}

#[derive(Debug, Clone)]
struct PlannedScope {
    scope_id: NodeId,
    kind: ScopeKind,
    variant: ScopeVariant,
    language: String,
    artifact_id: NodeId,
    owner_callable_id: Option<NodeId>,
    span: Option<SourceSpan>,
}

fn emit_declaration_scopes(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let language_by_artifact = language_by_artifact(graph);
    let callable_scope_by_declaration_span = callable_scope_by_declaration_span(graph);
    let existing_class_scope_by_span = class_scope_by_span(graph);
    let mut planned = Vec::new();

    for semantic in context.semantic_callables() {
        let artifact = semantic.artifact();
        let Some(language) = language_by_artifact.get(&artifact.artifact_id) else {
            continue;
        };
        for statement in semantic
            .statements()
            .iter()
            .filter(|statement| statement.owner_id == semantic.owner_id())
            .filter(|statement| {
                matches!(
                    statement.kind,
                    crate::ast::StatementKind::Function | crate::ast::StatementKind::Class
                )
            })
        {
            if statement.kind == crate::ast::StatementKind::Function
                && callable_scope_by_declaration_span
                    .contains_key(&(artifact.artifact_id.clone(), statement.source_span))
            {
                continue;
            }
            if statement.kind == crate::ast::StatementKind::Class
                && existing_class_scope_by_span
                    .contains_key(&(artifact.artifact_id.clone(), statement.source_span))
            {
                continue;
            }

            let (kind, variant) = match statement.kind {
                crate::ast::StatementKind::Function => {
                    (ScopeKind::Function, ScopeVariant::FunctionBody)
                }
                crate::ast::StatementKind::Class => (ScopeKind::Class, ScopeVariant::ClassBody),
                _ => unreachable!("filtered declaration scope kinds"),
            };
            planned.push(PlannedScope {
                scope_id: stable_id(
                    Tag::Scope,
                    crate::id_parts![
                        artifact.artifact_id,
                        "declaration",
                        scope_kind_key(kind),
                        semantic.callable().callable_id,
                        &span_key(statement.source_span),
                    ],
                ),
                kind,
                variant,
                language: language.clone(),
                artifact_id: artifact.artifact_id.clone(),
                owner_callable_id: Some(semantic.callable().callable_id.clone()),
                span: Some(statement.source_span),
            });
        }
    }

    insert_planned_scopes(graph, planned);
}

fn emit_comprehension_scopes(graph: &mut ProgramSupergraph) {
    let language_by_artifact = language_by_artifact(graph);
    let mut planned = Vec::new();

    for node in &graph.nodes {
        let NodeFact::Expression(expression) = &node.fact else {
            continue;
        };
        let Some(span) = node.span else {
            continue;
        };
        let Some(artifact_id) = node.owner.artifact_id.as_ref() else {
            continue;
        };
        let Some(language) = language_by_artifact.get(artifact_id) else {
            continue;
        };
        if !is_comprehension_expression(language, expression.original_text.as_deref()) {
            continue;
        }
        planned.push(PlannedScope {
            scope_id: stable_id(
                Tag::Scope,
                crate::id_parts![
                    artifact_id,
                    "comprehension",
                    expression.callable_id,
                    &span_key(span),
                ],
            ),
            kind: ScopeKind::Comprehension,
            variant: ScopeVariant::Comprehension,
            language: language.clone(),
            artifact_id: artifact_id.clone(),
            owner_callable_id: Some(expression.callable_id.clone()),
            span: Some(span),
        });
    }

    insert_planned_scopes(graph, planned);
}

fn insert_planned_scopes(graph: &mut ProgramSupergraph, planned: Vec<PlannedScope>) {
    let mut scopes_by_artifact = scope_infos_by_artifact(graph);
    for planned in planned {
        let parent_scope_id = nearest_parent_scope(
            scopes_by_artifact
                .get(&planned.artifact_id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            planned.span,
            planned.scope_id,
        );
        let binding_behavior = binding_behavior(planned.kind, planned.language.as_str());
        let owner = SourceOwnership {
            artifact_id: Some(planned.artifact_id.clone()),
            scope_id: parent_scope_id.clone(),
            callable_id: planned.owner_callable_id.clone(),
        };
        let inserted = graph.push_node_if_new(graph_node(
                planned.scope_id.clone(),
                NodeKind::Scope,
                owner.clone(),
                planned.span,
                Confidence::Exact,
                parser_evidence(
                    planned.artifact_id,
                    planned.span,
                    scope_kind_key(planned.kind),
                    "normalized lexical scope fact",
                ),
                NodeFact::Scope(Box::new(sg::Scope {
                    scope_id: planned.scope_id.clone(),
                    parent_scope_id: parent_scope_id.clone(),
                    artifact_id: planned.artifact_id.clone(),
                    kind: planned.kind,
                    variant: planned.variant,
                    language_variant: (language_variant(
                        planned.kind,
                        planned.variant,
                        Some(planned.language.as_str()),
                    )).map(Sym::from),
                    binding_behavior,
                    owner_callable_id: planned.owner_callable_id.clone(),
                    span: planned.span,
                })),
            ));
        if inserted {
            scopes_by_artifact
                .entry(planned.artifact_id.clone())
                .or_default()
                .push(ScopeInfo {
                    scope_id: planned.scope_id.clone(),
                    kind: planned.kind,
                    span: planned.span,
                    binding_behavior,
                });
        }
        if let Some(parent_scope_id) = parent_scope_id {
            add_contains_edge(
                graph,
                parent_scope_id,
                planned.scope_id,
                owner,
                planned.span,
                parser_evidence(
                    planned.artifact_id,
                    planned.span,
                    scope_kind_key(planned.kind),
                    "lexical scope containment",
                ),
            );
        }
    }
}

fn retarget_owned_facts_to_lexical_scopes(graph: &mut ProgramSupergraph) {
    let mut scopes_by_artifact = scope_infos_by_artifact(graph);
    for scopes in scopes_by_artifact.values_mut() {
        scopes.retain(|scope| scope.binding_behavior != ScopeBindingBehavior::Transparent);
    }

    for node in &mut graph.nodes {
        let Some(artifact_id) = node.owner.artifact_id.as_ref() else {
            continue;
        };
        let Some(span) = node.span else {
            continue;
        };
        if !matches!(
            node.fact,
            NodeFact::Statement(_)
                | NodeFact::Expression(_)
                | NodeFact::Condition(_)
                | NodeFact::CallSite(_)
        ) {
            continue;
        }
        let Some(scopes) = scopes_by_artifact.get(artifact_id) else {
            continue;
        };
        if let Some(scope_id) = nearest_scope_for_fact(scopes, span, &node.fact) {
            node.owner.scope_id = Some(scope_id);
        }
    }
}

fn emit_scope_member_edges(graph: &mut ProgramSupergraph) {
    let mut planned = Vec::new();
    for node in &graph.nodes {
        let Some(scope_id) = node.owner.scope_id.as_ref() else {
            continue;
        };
        if !matches!(
            node.fact,
            NodeFact::Statement(_)
                | NodeFact::Expression(_)
                | NodeFact::Condition(_)
                | NodeFact::CallSite(_)
        ) {
            continue;
        }
        if &node.node_id == scope_id {
            continue;
        }
        planned.push((
            scope_id.clone(),
            node.node_id.clone(),
            node.owner.clone(),
            node.span,
            node.evidence.clone(),
        ));
    }

    for (scope_id, member_id, owner, span, evidence) in planned {
        add_contains_edge(graph, scope_id, member_id, owner, span, evidence);
    }
}

fn add_contains_edge(
    graph: &mut ProgramSupergraph,
    container_id: NodeId,
    member_id: NodeId,
    owner: SourceOwnership,
    span: Option<SourceSpan>,
    evidence: Vec<Evidence>,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id("contains", container_id, member_id, CONTAINMENT_PRECISION),
            EdgeKind::Contains,
            container_id,
            member_id,
            owner,
            span,
            Confidence::Exact,
            evidence,
            EdgeFact::Contains(sg::Contains {
                container_id,
                member_id,
            }),
        ),
    );
}

fn scope_infos_by_artifact(graph: &ProgramSupergraph) -> HashMap<NodeId, Vec<ScopeInfo>> {
    let mut by_artifact = HashMap::<NodeId, Vec<ScopeInfo>>::new();
    for node in &graph.nodes {
        let NodeFact::Scope(scope) = &node.fact else {
            continue;
        };
        by_artifact
            .entry(scope.artifact_id.clone())
            .or_default()
            .push(ScopeInfo {
                scope_id: scope.scope_id.clone(),
                kind: scope.kind,
                span: scope.span,
                binding_behavior: scope.binding_behavior,
            });
    }
    by_artifact
}

/// `scopes` must already be restricted to the artifact of the scope being placed.
fn nearest_parent_scope(
    scopes: &[ScopeInfo],
    span: Option<SourceSpan>,
    scope_id: NodeId,
) -> Option<NodeId> {
    let Some(span) = span else {
        return scopes
            .iter()
            .find(|scope| scope.kind == ScopeKind::Module)
            .map(|scope| scope.scope_id.clone());
    };
    scopes
        .iter()
        .filter(|scope| scope.scope_id != scope_id)
        .filter(|scope| {
            scope
                .span
                .is_some_and(|candidate| span_contains(candidate, span) && candidate != span)
        })
        .min_by_key(|scope| {
            scope
                .span
                .map(|span| span.end_byte.saturating_sub(span.start_byte))
                .unwrap_or(usize::MAX as u32)
        })
        .map(|scope| scope.scope_id.clone())
        .or_else(|| {
            scopes
                .iter()
                .find(|scope| scope.kind == ScopeKind::Module)
                .map(|scope| scope.scope_id.clone())
        })
}

/// `scopes` must already be restricted to the artifact of the fact being placed.
fn nearest_scope_for_fact(
    scopes: &[ScopeInfo],
    span: SourceSpan,
    fact: &NodeFact,
) -> Option<NodeId> {
    scopes
        .iter()
        .filter(|scope| {
            scope.span.is_some_and(|candidate| {
                let contains = span_contains(candidate, span);
                if candidate != span {
                    return contains;
                }
                !matches!(
                    (&scope.kind, fact),
                    (
                        ScopeKind::Function | ScopeKind::Class,
                        NodeFact::Statement(_)
                    )
                )
            })
        })
        .min_by_key(|scope| {
            scope
                .span
                .map(|span| span.end_byte.saturating_sub(span.start_byte))
                .unwrap_or(usize::MAX as u32)
        })
        .map(|scope| scope.scope_id.clone())
}

fn statement_spans(graph: &ProgramSupergraph) -> BTreeMap<NodeId, StatementInfo> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeFact::Statement(statement) = &node.fact else {
                return None;
            };
            let span = node.span?;
            Some((statement.statement_id.clone(), StatementInfo { span }))
        })
        .collect()
}

fn span_for_statement_ids(
    statement_ids: &[NodeId],
    statements: &BTreeMap<NodeId, StatementInfo>,
) -> Option<SourceSpan> {
    statement_ids
        .iter()
        .filter_map(|statement_id| statements.get(statement_id).map(|statement| statement.span))
        .reduce(join_spans)
}

fn join_spans(left: SourceSpan, right: SourceSpan) -> SourceSpan {
    SourceSpan {
        start_byte: left.start_byte.min(right.start_byte),
        end_byte: left.end_byte.max(right.end_byte),
        start_row: left.start_row.min(right.start_row),
        start_column: if left.start_byte <= right.start_byte {
            left.start_column
        } else {
            right.start_column
        },
        end_row: left.end_row.max(right.end_row),
        end_column: if left.end_byte >= right.end_byte {
            left.end_column
        } else {
            right.end_column
        },
    }
}

fn language_by_artifact(graph: &ProgramSupergraph) -> BTreeMap<NodeId, String> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Artifact(artifact) => {
                Some((artifact.artifact_id.clone(), graph.language.clone()))
            }
            _ => None,
        })
        .collect()
}

fn callable_artifacts(graph: &ProgramSupergraph) -> BTreeMap<NodeId, NodeId> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Callable(callable) => {
                Some((callable.callable_id.clone(), callable.artifact_id.clone()))
            }
            _ => None,
        })
        .collect()
}

fn callable_scope_by_declaration_span(
    graph: &ProgramSupergraph,
) -> BTreeMap<(NodeId, SourceSpan), NodeId> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Callable(callable) => Some((
                (callable.artifact_id.clone(), callable.declaration_span),
                callable.scope_id.clone(),
            )),
            _ => None,
        })
        .collect()
}

fn class_scope_by_span(graph: &ProgramSupergraph) -> BTreeMap<(NodeId, SourceSpan), NodeId> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Scope(scope) if scope.kind == ScopeKind::Class => scope
                .span
                .map(|span| ((scope.artifact_id.clone(), span), scope.scope_id.clone())),
            _ => None,
        })
        .collect()
}

fn variant_for_existing_scope(kind: ScopeKind) -> ScopeVariant {
    match kind {
        ScopeKind::Module => ScopeVariant::FileModule,
        ScopeKind::Class => ScopeVariant::ClassBody,
        ScopeKind::Function => ScopeVariant::FunctionBody,
        ScopeKind::Block => ScopeVariant::StatementBlock,
        ScopeKind::Catch => ScopeVariant::CatchHandler,
        ScopeKind::Comprehension => ScopeVariant::Comprehension,
        ScopeKind::LanguageSpecific => ScopeVariant::LanguageSpecific,
    }
}

fn binding_behavior(kind: ScopeKind, language: &str) -> ScopeBindingBehavior {
    match (kind, language) {
        (ScopeKind::Block, "python") => ScopeBindingBehavior::Transparent,
        (ScopeKind::Catch, "python") => ScopeBindingBehavior::LanguageSpecific,
        (ScopeKind::LanguageSpecific, _) => ScopeBindingBehavior::LanguageSpecific,
        _ => ScopeBindingBehavior::Boundary,
    }
}

fn language_variant(
    kind: ScopeKind,
    variant: ScopeVariant,
    language: Option<&str>,
) -> Option<String> {
    let language = language?;
    let suffix = match (kind, variant, language) {
        (ScopeKind::Block, ScopeVariant::StatementBlock, "python") => "transparent-block",
        (ScopeKind::Block, ScopeVariant::StatementBlock, "typescript" | "rust") => "block",
        (ScopeKind::Catch, ScopeVariant::CatchHandler, "python") => "except-handler",
        (ScopeKind::Catch, ScopeVariant::CatchHandler, "typescript") => "catch-clause",
        (ScopeKind::Comprehension, ScopeVariant::Comprehension, "python") => "comprehension",
        (ScopeKind::Module, ScopeVariant::FileModule, _) => "module",
        (ScopeKind::Class, ScopeVariant::ClassBody, _) => "class-body",
        (ScopeKind::Function, ScopeVariant::FunctionBody, _) => "function-body",
        _ => "language-specific",
    };
    Some(format!("{language}:{suffix}"))
}

fn is_comprehension_expression(language: &str, text: Option<&str>) -> bool {
    if language != "python" {
        return false;
    }
    let Some(text) = text else {
        return false;
    };
    let trimmed = text.trim();
    (trimmed.starts_with('[') || trimmed.starts_with('{') || trimmed.starts_with('('))
        && trimmed.contains(" for ")
        && trimmed.contains(" in ")
}

fn parser_evidence(
    artifact_id: NodeId,
    span: Option<SourceSpan>,
    syntax_kind: &str,
    summary: &str,
) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Parser,
        summary: Sym::from(summary.to_string()),
        source_id: Some(artifact_id),
        source_span: span,
        content_hash: None,
        syntax: Some(SyntaxReference {
            kind: Sym::from(syntax_kind.to_string()),
            key_prefix: span.map(|_| Sym::new("scope")),
            field_path: Box::default(),
        }),
    }]
}

fn scope_kind_key(kind: ScopeKind) -> &'static str {
    match kind {
        ScopeKind::Module => "module",
        ScopeKind::Class => "class",
        ScopeKind::Function => "function",
        ScopeKind::Block => "block",
        ScopeKind::Catch => "catch",
        ScopeKind::Comprehension => "comprehension",
        ScopeKind::LanguageSpecific => "language-specific",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, ConditionAst, ConditionKind as AstConditionKind, ExpressionAst,
        ExpressionKind as AstExpressionKind, FileAst, ParamAst, ProjectAst, SourceSpan,
        StatementAst, StatementKind as AstStatementKind, SymbolAst, SymbolKind,
    };

    #[test]
    fn sg024_normalizes_python_scopes_without_making_control_blocks_binding_boundaries() {
        let graph = build_python_supergraph(&python_project());

        assert_scope_kind(&graph, ScopeKind::Module);
        assert_scope_kind(&graph, ScopeKind::Class);
        assert_scope_kind(&graph, ScopeKind::Function);
        assert_scope_kind(&graph, ScopeKind::Block);
        assert_scope_kind(&graph, ScopeKind::Catch);
        assert_scope_kind(&graph, ScopeKind::Comprehension);
        assert_scope_variant(
            &graph,
            ScopeKind::Block,
            Some("python:transparent-block"),
            ScopeBindingBehavior::Transparent,
        );
        assert_scope_variant(
            &graph,
            ScopeKind::Catch,
            Some("python:except-handler"),
            ScopeBindingBehavior::LanguageSpecific,
        );
        assert_scope_variant(
            &graph,
            ScopeKind::Comprehension,
            Some("python:comprehension"),
            ScopeBindingBehavior::Boundary,
        );

        let outer_scope = callable_scope(&graph, "sample.outer");
        let branch_assignment = statement_at(&graph, span(60, 70));
        assert_eq!(
            branch_assignment.owner.scope_id,
            Some(outer_scope)
        );

        let comprehension_scope = scope_at(&graph, ScopeKind::Comprehension, span(190, 215));
        let comprehension_expr = expression_at(&graph, span(190, 215));
        assert_eq!(
            comprehension_expr.owner.scope_id,
            Some(comprehension_scope.scope_id)
        );

        let nested_function_scope = scope_at(&graph, ScopeKind::Function, span(72, 95));
        assert_contains(
            &graph,
            nested_function_scope.parent_scope_id.unwrap(),
            nested_function_scope.scope_id,
        );
    }

    #[test]
    fn sg024_normalizes_typescript_block_and_catch_binding_scopes() {
        let graph = build_typescript_supergraph(&typescript_project());

        assert_scope_kind(&graph, ScopeKind::Module);
        assert_scope_kind(&graph, ScopeKind::Class);
        assert_scope_kind(&graph, ScopeKind::Function);
        assert_scope_variant(
            &graph,
            ScopeKind::Block,
            Some("typescript:block"),
            ScopeBindingBehavior::Boundary,
        );
        assert_scope_variant(
            &graph,
            ScopeKind::Catch,
            Some("typescript:catch-clause"),
            ScopeBindingBehavior::Boundary,
        );

        let block_scope = scope_at(&graph, ScopeKind::Block, span(60, 95));
        let branch_assignment = statement_at(&graph, span(60, 70));
        assert_eq!(
            branch_assignment.owner.scope_id,
            Some(block_scope.scope_id)
        );

        let nested_function_scope = scope_at(&graph, ScopeKind::Function, span(72, 95));
        assert_eq!(
            nested_function_scope.parent_scope_id,
            Some(block_scope.scope_id)
        );
        assert_contains(
            &graph,
            block_scope.scope_id,
            nested_function_scope.scope_id,
        );

        let catch_scope = scope_at(&graph, ScopeKind::Catch, span(136, 145));
        let handler_statement = statement_at(&graph, span(136, 145));
        assert_eq!(
            handler_statement.owner.scope_id,
            Some(catch_scope.scope_id)
        );
    }

    fn assert_scope_kind(graph: &ProgramSupergraph, kind: ScopeKind) {
        assert!(
            scopes(graph).iter().any(|scope| scope.kind == kind),
            "missing scope kind {kind:?}"
        );
    }

    fn assert_scope_variant(
        graph: &ProgramSupergraph,
        kind: ScopeKind,
        language_variant: Option<&str>,
        binding_behavior: ScopeBindingBehavior,
    ) {
        assert!(
            scopes(graph).iter().any(|scope| {
                scope.kind == kind
                    && scope.language_variant.as_deref() == language_variant
                    && scope.binding_behavior == binding_behavior
            }),
            "missing {kind:?} scope variant {language_variant:?} with {binding_behavior:?}"
        );
    }

    fn callable_scope<'a>(graph: &'a ProgramSupergraph, qualified_name: &str) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Callable(callable) if callable.qualified_name == qualified_name => {
                    Some(callable.scope_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing callable {qualified_name}"))
    }

    fn scope_at(graph: &ProgramSupergraph, kind: ScopeKind, span: SourceSpan) -> &sg::Scope {
        scopes(graph)
            .into_iter()
            .find(|scope| scope.kind == kind && scope.span == Some(span))
            .unwrap_or_else(|| panic!("missing {kind:?} scope at {span:?}"))
    }

    fn statement_at(graph: &ProgramSupergraph, span: SourceSpan) -> &crate::supergraph::GraphNode {
        graph
            .nodes
            .iter()
            .find(|node| matches!(node.fact, NodeFact::Statement(_)) && node.span == Some(span))
            .unwrap_or_else(|| panic!("missing statement at {span:?}"))
    }

    fn expression_at(graph: &ProgramSupergraph, span: SourceSpan) -> &crate::supergraph::GraphNode {
        graph
            .nodes
            .iter()
            .find(|node| matches!(node.fact, NodeFact::Expression(_)) && node.span == Some(span))
            .unwrap_or_else(|| panic!("missing expression at {span:?}"))
    }

    fn assert_contains(graph: &ProgramSupergraph, container_id: NodeId, member_id: NodeId) {
        assert!(
            graph.edges.iter().any(|edge| {
                edge.kind == EdgeKind::Contains
                    && edge.source_id == container_id
                    && edge.target_id == Some(member_id)
            }),
            "missing Contains edge {container_id} -> {member_id}"
        );
    }

    fn scopes(graph: &ProgramSupergraph) -> Vec<&sg::Scope> {
        graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Scope(scope) => Some(scope.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn python_project() -> ProjectAst {
        scoped_project("sample.py", "sample:<module>", "sample:outer", true)
    }

    fn typescript_project() -> ProjectAst {
        scoped_project("sample.ts", "sample:<module>", "sample:outer", false)
    }

    fn scoped_project(
        path: &str,
        module_owner_id: &str,
        function_owner_id: &str,
        python: bool,
    ) -> ProjectAst {
        ProjectAst {
            manifests: Vec::new(),
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: vec![
                    SymbolAst {
                        id: "sample:Example".to_string(),
                        name: "Example".to_string(),
                        kind: SymbolKind::Class,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: Vec::new(),
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(5, 15)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: Vec::new(),
                        statements: Vec::new(),
                        expressions: Vec::new(),
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(0, 20),
                    },
                    SymbolAst {
                        id: function_owner_id.to_string(),
                        name: "outer".to_string(),
                        kind: SymbolKind::Function,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: vec![ParamAst {
                            name: "items".to_string(),
                            text: "items".to_string(),
                            source_span: span(36, 41),
                        }],
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(40, 250)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: Vec::new(),
                        statements: function_statements(function_owner_id, python),
                        expressions: function_expressions(function_owner_id, python),
                        conditions: vec![ConditionAst {
                            kind: AstConditionKind::If,
                            text: "flag".to_string(),
                            owner_id: function_owner_id.to_string(),
                            source_span: span(53, 57),
                        }],
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(30, 260),
                    },
                ],
                statements: vec![
                    StatementAst {
                        kind: AstStatementKind::Class,
                        text: "class Example".to_string(),
                        owner_id: module_owner_id.to_string(),
                        source_span: span(0, 20),
                    },
                    StatementAst {
                        kind: AstStatementKind::Function,
                        text: "def outer".to_string(),
                        owner_id: module_owner_id.to_string(),
                        source_span: span(30, 260),
                    },
                ],
                expressions: Vec::new(),
                conditions: Vec::new(),
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

    fn function_statements(owner_id: &str, python: bool) -> Vec<StatementAst> {
        vec![
            statement(AstStatementKind::If, "if flag", owner_id, span(50, 100)),
            statement(
                AstStatementKind::Assignment,
                "value = 1",
                owner_id,
                span(60, 70),
            ),
            statement(
                AstStatementKind::Function,
                "def inner",
                owner_id,
                span(72, 95),
            ),
            statement(AstStatementKind::Try, "try", owner_id, span(110, 170)),
            statement(
                if python {
                    AstStatementKind::Raise
                } else {
                    AstStatementKind::Throw
                },
                if python { "raise err" } else { "throw err" },
                owner_id,
                span(120, 125),
            ),
            statement(AstStatementKind::Catch, "catch", owner_id, span(130, 150)),
            statement(
                AstStatementKind::Expression,
                "recover()",
                owner_id,
                span(136, 145),
            ),
            statement(
                AstStatementKind::Finally,
                "finally",
                owner_id,
                span(155, 165),
            ),
            statement(
                AstStatementKind::Expression,
                "values = [item for item in items]",
                owner_id,
                span(180, 220),
            ),
        ]
    }

    fn function_expressions(owner_id: &str, python: bool) -> Vec<ExpressionAst> {
        let mut expressions = vec![ExpressionAst {
            kind: AstExpressionKind::Identifier,
            text: "flag".to_string(),
            owner_id: owner_id.to_string(),
            source_span: span(53, 57),
        }];
        if python {
            expressions.push(ExpressionAst {
                kind: AstExpressionKind::Unknown,
                text: "[item for item in items]".to_string(),
                owner_id: owner_id.to_string(),
                source_span: span(190, 215),
            });
        }
        expressions
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

    fn span(start_byte: u32, end_byte: u32) -> SourceSpan {
        SourceSpan {
            start_byte,
            end_byte,
            start_row: start_byte as u32,
            start_column: 0,
            end_row: end_byte as u32,
            end_column: 0,
        }
    }
}
