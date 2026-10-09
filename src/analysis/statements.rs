use crate::intern::Sym;
use std::collections::BTreeMap;

use crate::ast::{StatementAst, StatementKind as AstStatementKind};
use crate::supergraph::{
    self as sg, Confidence, EdgeFact, EdgeKind, Evidence, EvidenceKind, NodeFact, NodeId, NodeKind,
    ProgramSupergraph, SourceOwnership, SyntaxReference, stable_edge_id, stable_id,
};

use crate::id_parts;
use crate::supergraph::ids::Tag;
use super::{
    SemanticCallable, SemanticContext, graph_edge, graph_node, insert_edge, insert_node,
    span_contains, span_key,
};

const CONTAINMENT_PRECISION: &str = "sg020-statement-containment";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    for semantic in context.semantic_callables() {
        emit_callable(graph, &semantic);
    }
}

fn emit_callable(graph: &mut ProgramSupergraph, semantic: &SemanticCallable<'_>) {
    let statements = ordered_statement_facts(semantic);
    if statements.is_empty() {
        return;
    }

    let parents = parent_indexes(&statements);
    let ordinals = sibling_ordinals(&statements, &parents);
    let statement_ids = statement_ids(semantic, &statements, &ordinals);
    let child_ids = child_statement_ids(&parents, &statement_ids);
    let owner = statement_owner(semantic);

    for (index, statement) in statements.iter().enumerate() {
        let statement_id = statement_ids[index].clone();
        let parent_statement_id = parents[index].map(|parent| statement_ids[parent].clone());
        let evidence = parser_evidence(semantic, statement, "normalized statement fact");
        insert_node(
            graph,
            graph_node(
                statement_id,
                NodeKind::Statement,
                owner.clone(),
                Some(statement.source_span),
                Confidence::Exact,
                evidence,
                NodeFact::Statement(sg::Statement {
                    statement_id,
                    callable_id: semantic.callable().callable_id,
                    parent_statement_id,
                    kind: statement_kind(statement.kind),
                    ordinal: ordinals[index],
                    child_statement_ids: child_ids.get(&index).cloned().unwrap_or_default(),
                    expression_ids: Vec::new(),
                    control_effects: Vec::new(),
                }),
            ),
        );
    }

    for (index, statement_id) in statement_ids.iter().enumerate() {
        let container_id = parents[index]
            .map(|parent| statement_ids[parent].clone())
            .unwrap_or_else(|| semantic.callable().callable_id);
        add_contains_edge(
            graph,
            semantic,
            container_id,
            *statement_id,
            &statements[index],
            owner.clone(),
        );
    }
}

fn ordered_statement_facts<'a>(semantic: &'a SemanticCallable<'_>) -> Vec<&'a StatementAst> {
    let mut statements = semantic
        .statements()
        .iter()
        .filter(|statement| statement.owner_id == semantic.owner_id())
        .collect::<Vec<_>>();
    statements.sort_by(|left, right| {
        (
            left.source_span.start_byte,
            left.source_span.end_byte,
            statement_kind(left.kind),
            left.text.as_str(),
        )
            .cmp(&(
                right.source_span.start_byte,
                right.source_span.end_byte,
                statement_kind(right.kind),
                right.text.as_str(),
            ))
    });
    statements.dedup_by(|left, right| {
        left.source_span == right.source_span && left.kind == right.kind && left.text == right.text
    });
    statements
}

fn parent_indexes(statements: &[&StatementAst]) -> Vec<Option<usize>> {
    statements
        .iter()
        .enumerate()
        .map(|(index, statement)| {
            statements
                .iter()
                .enumerate()
                .filter(|(candidate_index, candidate)| {
                    *candidate_index != index
                        && candidate.source_span != statement.source_span
                        && span_contains(candidate.source_span, statement.source_span)
                })
                .min_by_key(|(_, candidate)| {
                    candidate
                        .source_span
                        .end_byte
                        .saturating_sub(candidate.source_span.start_byte)
                })
                .map(|(parent_index, _)| parent_index)
        })
        .collect()
}

fn sibling_ordinals(statements: &[&StatementAst], parents: &[Option<usize>]) -> Vec<usize> {
    let mut siblings = BTreeMap::<Option<usize>, Vec<usize>>::new();
    for (index, parent) in parents.iter().copied().enumerate() {
        siblings.entry(parent).or_default().push(index);
    }

    let mut ordinals = vec![0; statements.len()];
    for sibling_indexes in siblings.values_mut() {
        sibling_indexes.sort_by(|left, right| {
            (
                statements[*left].source_span.start_byte,
                statements[*left].source_span.end_byte,
                statement_kind(statements[*left].kind),
                statements[*left].text.as_str(),
            )
                .cmp(&(
                    statements[*right].source_span.start_byte,
                    statements[*right].source_span.end_byte,
                    statement_kind(statements[*right].kind),
                    statements[*right].text.as_str(),
                ))
        });
        for (ordinal, statement_index) in sibling_indexes.iter().copied().enumerate() {
            ordinals[statement_index] = ordinal;
        }
    }
    ordinals
}

fn statement_ids(
    semantic: &SemanticCallable<'_>,
    statements: &[&StatementAst],
    ordinals: &[usize],
) -> Vec<NodeId> {
    statements
        .iter()
        .enumerate()
        .map(|(index, statement)| {
            stable_id(Tag::Statement,
                id_parts![
                    &semantic.callable().callable_id,
                    statement_kind_key(statement.kind),
                    &span_key(statement.source_span),
                    &ordinals[index].to_string(),
                ],
            )
        })
        .collect()
}

fn child_statement_ids(
    parents: &[Option<usize>],
    statement_ids: &[NodeId],
) -> BTreeMap<usize, Vec<NodeId>> {
    let mut child_ids = BTreeMap::<usize, Vec<NodeId>>::new();
    for (index, parent) in parents.iter().copied().enumerate() {
        if let Some(parent) = parent {
            child_ids
                .entry(parent)
                .or_default()
                .push(statement_ids[index].clone());
        }
    }
    child_ids
}

fn add_contains_edge(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    container_id: NodeId,
    member_id: NodeId,
    statement: &StatementAst,
    owner: SourceOwnership,
) {
    insert_edge(
        graph,
        graph_edge(
            stable_edge_id(id_parts!["contains", container_id, member_id, CONTAINMENT_PRECISION],
            ),
            EdgeKind::Contains,
            container_id,
            member_id,
            owner,
            None,
            Confidence::Exact,
            parser_evidence(semantic, statement, "normalized statement containment"),
            EdgeFact::Contains(sg::Contains {
                container_id: container_id,
                member_id: member_id,
            }),
        ),
    );
}

fn statement_owner(semantic: &SemanticCallable<'_>) -> SourceOwnership {
    SourceOwnership {
        artifact_id: Some(semantic.artifact().artifact_id),
        scope_id: Some(semantic.callable().scope_id),
        callable_id: Some(semantic.callable().callable_id),
    }
}

fn parser_evidence(
    semantic: &SemanticCallable<'_>,
    statement: &StatementAst,
    summary: &str,
) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Parser,
        summary: Sym::from(summary.to_string()),
        source_id: Some(semantic.artifact().artifact_id),
        source_span: Some(statement.source_span),
        content_hash: semantic.artifact().content_hash.clone(),
        syntax: Some(SyntaxReference {
            kind: Sym::from(statement_kind_key(statement.kind).to_string()),
            key_prefix: Some(Sym::new("statement")),
            field_path: Box::default(),
        }),
    }]
}

fn statement_kind(kind: AstStatementKind) -> sg::StatementKind {
    match kind {
        AstStatementKind::Assignment => sg::StatementKind::Assignment,
        AstStatementKind::Declaration => sg::StatementKind::Declaration,
        AstStatementKind::Expression => sg::StatementKind::Expression,
        AstStatementKind::If => sg::StatementKind::Branch,
        AstStatementKind::Loop => sg::StatementKind::Loop,
        AstStatementKind::Return => sg::StatementKind::Return,
        AstStatementKind::Raise => sg::StatementKind::Raise,
        AstStatementKind::Throw => sg::StatementKind::Throw,
        AstStatementKind::Function => sg::StatementKind::Function,
        AstStatementKind::Class => sg::StatementKind::Class,
        AstStatementKind::Break => sg::StatementKind::Break,
        AstStatementKind::Continue => sg::StatementKind::Continue,
        AstStatementKind::Try => sg::StatementKind::Try,
        AstStatementKind::Catch => sg::StatementKind::Catch,
        AstStatementKind::Finally => sg::StatementKind::Finally,
        AstStatementKind::Unknown => sg::StatementKind::Unknown,
    }
}

fn statement_kind_key(kind: AstStatementKind) -> &'static str {
    match statement_kind(kind) {
        sg::StatementKind::Declaration => "declaration",
        sg::StatementKind::Assignment => "assignment",
        sg::StatementKind::Expression => "expression",
        sg::StatementKind::Branch => "branch",
        sg::StatementKind::Loop => "loop",
        sg::StatementKind::Return => "return",
        sg::StatementKind::Raise => "raise",
        sg::StatementKind::Throw => "throw",
        sg::StatementKind::Function => "function",
        sg::StatementKind::Class => "class",
        sg::StatementKind::Break => "break",
        sg::StatementKind::Continue => "continue",
        sg::StatementKind::Try => "try",
        sg::StatementKind::Catch => "catch",
        sg::StatementKind::Finally => "finally",
        sg::StatementKind::Import => "import",
        sg::StatementKind::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use crate::supergraph::ids::NodeId;
    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, FileAst, ProjectAst, SourceSpan, StatementAst, StatementKind as AstStatementKind,
    };
    use crate::supergraph::{EdgeFact, EdgeKind, NodeFact, NodeKind, StatementKind};

    #[test]
    fn sg020_lowers_python_statements_with_containment_and_ordering() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));

        assert_statement_kinds(
            &graph,
            &[
                StatementKind::Declaration,
                StatementKind::Assignment,
                StatementKind::Expression,
                StatementKind::Branch,
                StatementKind::Loop,
                StatementKind::Return,
                StatementKind::Raise,
                StatementKind::Function,
                StatementKind::Class,
                StatementKind::Unknown,
            ],
        );
        assert_statement_structure(&graph);
    }

    #[test]
    fn sg020_lowers_typescript_statements_with_consistent_kinds() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));

        assert_statement_kinds(
            &graph,
            &[
                StatementKind::Declaration,
                StatementKind::Assignment,
                StatementKind::Expression,
                StatementKind::Branch,
                StatementKind::Loop,
                StatementKind::Return,
                StatementKind::Throw,
                StatementKind::Function,
                StatementKind::Class,
                StatementKind::Unknown,
            ],
        );
        assert_statement_structure(&graph);
    }

    fn assert_statement_kinds(
        graph: &crate::supergraph::ProgramSupergraph,
        expected: &[StatementKind],
    ) {
        let kinds = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Statement(statement) => Some(statement.kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        for kind in expected {
            assert!(kinds.contains(kind), "missing statement kind {kind:?}");
        }
    }

    fn assert_statement_structure(graph: &crate::supergraph::ProgramSupergraph) {
        let statements = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Statement(statement) => Some((node, statement)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let branch = statements
            .iter()
            .find(|(_, statement)| statement.kind == StatementKind::Branch)
            .expect("branch statement");
        assert_eq!(branch.1.ordinal, 3);
        assert_eq!(branch.1.child_statement_ids.len(), 2);

        let child_ordinals = branch
            .1
            .child_statement_ids
            .iter()
            .map(|child_id| {
                statements
                    .iter()
                    .find(|(_, statement)| &statement.statement_id == child_id)
                    .map(|(_, statement)| statement.ordinal)
                    .expect("child statement")
            })
            .collect::<Vec<_>>();
        assert_eq!(child_ordinals, vec![0, 1]);

        for (node, statement) in &statements {
            assert_eq!(node.kind, NodeKind::Statement);
            assert!(node.owner.artifact_id.is_some());
            assert!(node.owner.scope_id.is_some());
            assert_eq!(
                node.owner.callable_id.as_ref(),
                Some(&statement.callable_id)
            );
            assert_eq!(
                node.span,
                Some(span_for_statement(statement.kind, statement.ordinal))
            );
            assert!(!node.fact_id.is_none());
            assert!(!node.payload_hash.is_none());
            assert!(
                node.evidence
                    .iter()
                    .any(|evidence| evidence.syntax.is_some())
            );
        }

        let contains_edges = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Contains)
            .filter_map(|edge| match &edge.fact {
                EdgeFact::Contains(contains) => Some(contains),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            contains_edges
                .iter()
                .any(|edge| edge.container_id == crate::supergraph::ids::callable_id_from_text("sample:<module>"))
        );
        assert!(branch.1.child_statement_ids.iter().all(|child_id| {
            contains_edges.iter().any(|edge| {
                edge.container_id == branch.1.statement_id && edge.member_id == *child_id
            })
        }));
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
                statements: statements(owner_id, python),
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

    fn statements(owner_id: &str, python: bool) -> Vec<StatementAst> {
        vec![
            statement(
                AstStatementKind::Declaration,
                "declare value",
                owner_id,
                span(0, 5),
            ),
            statement(
                AstStatementKind::Assignment,
                "value = 1",
                owner_id,
                span(10, 20),
            ),
            statement(
                AstStatementKind::Expression,
                "effect()",
                owner_id,
                span(21, 30),
            ),
            statement(AstStatementKind::If, "if value", owner_id, span(31, 80)),
            statement(
                AstStatementKind::Assignment,
                "nested = value",
                owner_id,
                span(40, 45),
            ),
            statement(
                AstStatementKind::Return,
                "return nested",
                owner_id,
                span(50, 55),
            ),
            statement(
                AstStatementKind::Loop,
                "while value",
                owner_id,
                span(81, 120),
            ),
            statement(
                AstStatementKind::Expression,
                "tick()",
                owner_id,
                span(90, 95),
            ),
            statement(
                if python {
                    AstStatementKind::Raise
                } else {
                    AstStatementKind::Throw
                },
                if python { "raise Error" } else { "throw error" },
                owner_id,
                span(100, 105),
            ),
            statement(
                AstStatementKind::Function,
                "function helper",
                owner_id,
                span(121, 150),
            ),
            statement(
                AstStatementKind::Class,
                "class Example",
                owner_id,
                span(151, 180),
            ),
            statement(
                AstStatementKind::Unknown,
                "unknown",
                owner_id,
                span(181, 190),
            ),
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

    fn span_for_statement(kind: StatementKind, ordinal: usize) -> SourceSpan {
        match (kind, ordinal) {
            (StatementKind::Declaration, 0) => span(0, 5),
            (StatementKind::Assignment, 1) => span(10, 20),
            (StatementKind::Expression, 2) => span(21, 30),
            (StatementKind::Branch, 3) => span(31, 80),
            (StatementKind::Assignment, 0) => span(40, 45),
            (StatementKind::Return, 1) => span(50, 55),
            (StatementKind::Loop, 4) => span(81, 120),
            (StatementKind::Expression, 0) => span(90, 95),
            (StatementKind::Raise | StatementKind::Throw, 1) => span(100, 105),
            (StatementKind::Function, 5) => span(121, 150),
            (StatementKind::Class, 6) => span(151, 180),
            (StatementKind::Unknown, 7) => span(181, 190),
            unexpected => panic!("unexpected statement shape {unexpected:?}"),
        }
    }

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start as u32,
            end_byte: end as u32,
            start_row: start as u32,
            start_column: 0,
            end_row: end as u32,
            end_column: 0,
        }
    }
}
