use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::{ExpressionAst, ExpressionKind as AstExpressionKind};
use crate::supergraph::{
    self as sg, Confidence, EdgeFact, EdgeKind, Evidence, EvidenceKind, NodeFact, NodeId, NodeKind,
    NormalizedExpression, ProgramSupergraph, SourceOwnership, SyntaxReference, ValueLiteral,
    expression_id,
};

use super::{
    SemanticCallable, SemanticContext, callable_index::CallableIndex, graph_edge, graph_node,
    insert_edge, insert_node, span_contains, span_key, stable_id,
};

const CONTAINMENT_PRECISION: &str = "sg021-expression-containment";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let index = CallableIndex::build(graph);
    for semantic in context.semantic_callables() {
        emit_callable(graph, &index, &semantic);
    }
}

fn emit_callable(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
) {
    let expressions = ordered_expression_facts(semantic);
    if expressions.is_empty() {
        clear_statement_expression_ids(
            graph,
            index,
            &semantic.callable().callable_id,
            &statement_ids_for_callable(graph, index, semantic),
        );
        return;
    }

    let statement_ids = statement_ids_for_callable(graph, index, semantic);
    let parent_indexes = parent_indexes(&expressions);
    let statement_indexes = statement_indexes(&expressions, semantic);
    let ordinals = sibling_ordinals(&expressions, &parent_indexes, &statement_indexes);
    let expression_ids = expression_ids(semantic, &expressions, &ordinals);
    let child_ids = child_expression_ids(&parent_indexes, &expression_ids);
    let statement_expression_ids =
        statement_expression_ids(&parent_indexes, &statement_indexes, &expression_ids);
    let owner = expression_owner(semantic);

    update_statement_expression_ids(
        graph,
        index,
        &semantic.callable().callable_id,
        &statement_ids,
        &statement_expression_ids,
    );

    for (index, expression) in expressions.iter().enumerate() {
        let expression_id = expression_ids[index].clone();
        let statement_id = statement_indexes[index]
            .and_then(|statement_index| statement_ids.get(statement_index).cloned());
        let parent_expression_id =
            parent_indexes[index].map(|parent| expression_ids[parent].clone());
        let kind = expression_kind(expression.kind);
        let confidence = if kind == sg::ExpressionKind::Unknown {
            Confidence::Unknown
        } else {
            Confidence::Exact
        };

        insert_node(
            graph,
            graph_node(
                expression_id.clone(),
                NodeKind::Expression,
                owner.clone(),
                Some(expression.source_span),
                confidence,
                parser_evidence(semantic, expression, "normalized expression fact"),
                NodeFact::Expression(sg::Expression {
                    expression_id,
                    callable_id: semantic.callable().callable_id.clone(),
                    statement_id,
                    parent_expression_id,
                    kind,
                    ordinal: ordinals[index],
                    child_expression_ids: child_ids.get(&index).cloned().unwrap_or_default(),
                    symbol_id: None,
                    value_id: None,
                    original_text: Some(expression.text.clone()),
                    normalized: normalized_expression(semantic, expression),
                }),
            ),
        );
    }

    for (index, expression_id) in expression_ids.iter().enumerate() {
        let Some(container_id) = parent_indexes[index]
            .map(|parent| expression_ids[parent].clone())
            .or_else(|| {
                statement_indexes[index].and_then(|statement| statement_ids.get(statement).cloned())
            })
        else {
            continue;
        };
        add_contains_edge(
            graph,
            semantic,
            &container_id,
            expression_id,
            &expressions[index],
            owner.clone(),
        );
    }
}

fn ordered_expression_facts<'a>(semantic: &'a SemanticCallable<'_>) -> Vec<&'a ExpressionAst> {
    let mut expressions = semantic
        .expressions()
        .iter()
        .filter(|expression| expression.owner_id == semantic.owner_id())
        .collect::<Vec<_>>();
    expressions.sort_by(|left, right| expression_sort_key(left).cmp(&expression_sort_key(right)));
    expressions.dedup_by(|left, right| {
        left.source_span == right.source_span && left.kind == right.kind && left.text == right.text
    });
    expressions
}

fn parent_indexes(expressions: &[&ExpressionAst]) -> Vec<Option<usize>> {
    expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            expressions
                .iter()
                .enumerate()
                .filter(|(candidate_index, candidate)| {
                    *candidate_index != index
                        && candidate.source_span != expression.source_span
                        && span_contains(candidate.source_span, expression.source_span)
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

fn statement_indexes(
    expressions: &[&ExpressionAst],
    semantic: &SemanticCallable<'_>,
) -> Vec<Option<usize>> {
    let statements = semantic.statements();
    expressions
        .iter()
        .map(|expression| {
            statements
                .iter()
                .enumerate()
                .filter(|(_, statement)| {
                    statement.owner_id == semantic.owner_id()
                        && span_contains(statement.source_span, expression.source_span)
                })
                .min_by_key(|(_, statement)| {
                    statement
                        .source_span
                        .end_byte
                        .saturating_sub(statement.source_span.start_byte)
                })
                .map(|(statement_index, _)| statement_index)
        })
        .collect()
}

fn sibling_ordinals(
    expressions: &[&ExpressionAst],
    parent_indexes: &[Option<usize>],
    statement_indexes: &[Option<usize>],
) -> Vec<usize> {
    let mut siblings = BTreeMap::<(Option<usize>, Option<usize>), Vec<usize>>::new();
    for (index, parent) in parent_indexes.iter().copied().enumerate() {
        siblings
            .entry((parent, statement_indexes[index]))
            .or_default()
            .push(index);
    }

    let mut ordinals = vec![0; expressions.len()];
    for sibling_indexes in siblings.values_mut() {
        sibling_indexes.sort_by(|left, right| {
            expression_sort_key(expressions[*left]).cmp(&expression_sort_key(expressions[*right]))
        });
        for (ordinal, expression_index) in sibling_indexes.iter().copied().enumerate() {
            ordinals[expression_index] = ordinal;
        }
    }
    ordinals
}

fn expression_ids(
    semantic: &SemanticCallable<'_>,
    expressions: &[&ExpressionAst],
    ordinals: &[usize],
) -> Vec<NodeId> {
    expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            expression_id(
                &semantic.callable().callable_id,
                expression_kind_key(expression.kind),
                expression.source_span,
                ordinals[index],
            )
        })
        .collect()
}

fn child_expression_ids(
    parent_indexes: &[Option<usize>],
    expression_ids: &[NodeId],
) -> BTreeMap<usize, Vec<NodeId>> {
    let mut child_ids = BTreeMap::<usize, Vec<NodeId>>::new();
    for (index, parent) in parent_indexes.iter().copied().enumerate() {
        if let Some(parent) = parent {
            child_ids
                .entry(parent)
                .or_default()
                .push(expression_ids[index].clone());
        }
    }
    child_ids
}

fn statement_expression_ids(
    parent_indexes: &[Option<usize>],
    statement_indexes: &[Option<usize>],
    expression_ids: &[NodeId],
) -> BTreeMap<usize, Vec<NodeId>> {
    let mut ids = BTreeMap::<usize, Vec<NodeId>>::new();
    for (index, statement_index) in statement_indexes.iter().copied().enumerate() {
        if parent_indexes[index].is_some() {
            continue;
        }
        if let Some(statement_index) = statement_index {
            ids.entry(statement_index)
                .or_default()
                .push(expression_ids[index].clone());
        }
    }
    ids
}

fn statement_ids_for_callable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
) -> Vec<NodeId> {
    let mut statement_id_by_span = HashMap::new();
    for node in index.nodes(graph, &semantic.callable().callable_id) {
        if let NodeFact::Statement(statement) = &node.fact {
            statement_id_by_span
                .entry(node.span)
                .or_insert_with(|| statement.statement_id.clone());
        }
    }

    semantic
        .statements()
        .iter()
        .map(|statement| {
            statement_id_by_span
                .get(&Some(statement.source_span))
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

fn update_statement_expression_ids(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    statement_ids: &[NodeId],
    expression_ids_by_statement: &BTreeMap<usize, Vec<NodeId>>,
) {
    let mut statement_index_by_id = HashMap::new();
    for (statement_index, statement_id) in statement_ids.iter().enumerate() {
        statement_index_by_id
            .entry(statement_id.as_str())
            .or_insert(statement_index);
    }
    for &position in index.positions(callable_id) {
        let NodeFact::Statement(statement) = &mut graph.nodes[position].fact else {
            continue;
        };
        let Some(statement_index) = statement_index_by_id
            .get(statement.statement_id.as_str())
            .copied()
        else {
            continue;
        };
        statement.expression_ids = expression_ids_by_statement
            .get(&statement_index)
            .cloned()
            .unwrap_or_default();
    }
}

fn clear_statement_expression_ids(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    statement_ids: &[NodeId],
) {
    let statement_ids = statement_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    for &position in index.positions(callable_id) {
        let NodeFact::Statement(statement) = &mut graph.nodes[position].fact else {
            continue;
        };
        if statement_ids.contains(statement.statement_id.as_str()) {
            statement.expression_ids.clear();
        }
    }
}

fn add_contains_edge(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    container_id: &str,
    member_id: &str,
    expression: &ExpressionAst,
    owner: SourceOwnership,
) {
    insert_edge(
        graph,
        graph_edge(
            stable_id(
                "edge",
                &["contains", container_id, member_id, CONTAINMENT_PRECISION],
            ),
            EdgeKind::Contains,
            container_id.to_string(),
            member_id.to_string(),
            owner,
            None,
            Confidence::Exact,
            parser_evidence(semantic, expression, "normalized expression containment"),
            EdgeFact::Contains(sg::Contains {
                container_id: container_id.to_string(),
                member_id: member_id.to_string(),
            }),
        ),
    );
}

fn expression_owner(semantic: &SemanticCallable<'_>) -> SourceOwnership {
    SourceOwnership {
        artifact_id: Some(semantic.artifact().artifact_id.clone()),
        scope_id: Some(semantic.callable().scope_id.clone()),
        callable_id: Some(semantic.callable().callable_id.clone()),
    }
}

fn parser_evidence(
    semantic: &SemanticCallable<'_>,
    expression: &ExpressionAst,
    summary: &str,
) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Parser,
        summary: summary.to_string(),
        source_id: Some(semantic.artifact().artifact_id.clone()),
        source_span: Some(expression.source_span),
        content_hash: semantic.artifact().content_hash.clone(),
        syntax: Some(SyntaxReference {
            kind: expression_kind_key(expression.kind).to_string(),
            node_key: Some(format!("expression:{}", span_key(expression.source_span))),
            field_path: Vec::new(),
        }),
    }]
}

fn expression_kind(kind: AstExpressionKind) -> sg::ExpressionKind {
    match kind {
        AstExpressionKind::Identifier => sg::ExpressionKind::Identifier,
        AstExpressionKind::Literal => sg::ExpressionKind::Literal,
        AstExpressionKind::Call => sg::ExpressionKind::Call,
        AstExpressionKind::FieldAccess => sg::ExpressionKind::FieldAccess,
        AstExpressionKind::IndexAccess => sg::ExpressionKind::IndexAccess,
        AstExpressionKind::Assignment => sg::ExpressionKind::Assignment,
        AstExpressionKind::BinaryOperator => sg::ExpressionKind::BinaryOperator,
        AstExpressionKind::UnaryOperator => sg::ExpressionKind::UnaryOperator,
        AstExpressionKind::Conditional => sg::ExpressionKind::Conditional,
        AstExpressionKind::Await => sg::ExpressionKind::Await,
        AstExpressionKind::Yield => sg::ExpressionKind::Yield,
        AstExpressionKind::Unknown => sg::ExpressionKind::Unknown,
    }
}

fn expression_kind_key(kind: AstExpressionKind) -> &'static str {
    match expression_kind(kind) {
        sg::ExpressionKind::Identifier => "identifier",
        sg::ExpressionKind::Literal => "literal",
        sg::ExpressionKind::Call => "call",
        sg::ExpressionKind::FieldAccess => "field-access",
        sg::ExpressionKind::IndexAccess => "index-access",
        sg::ExpressionKind::Assignment => "assignment",
        sg::ExpressionKind::UnaryOperator => "unary-operator",
        sg::ExpressionKind::BinaryOperator => "binary-operator",
        sg::ExpressionKind::Conditional => "conditional",
        sg::ExpressionKind::Await => "await",
        sg::ExpressionKind::Yield => "yield",
        sg::ExpressionKind::Lambda => "lambda",
        sg::ExpressionKind::Unknown => "unknown",
    }
}

fn expression_sort_key(expression: &ExpressionAst) -> (usize, usize, sg::ExpressionKind, &str) {
    (
        expression.source_span.start_byte,
        expression.source_span.end_byte,
        expression_kind(expression.kind),
        expression.text.as_str(),
    )
}

const BINARY_OPERATORS: [&str; 31] = [
    "&&", "||", "??", "===", "!==", ">>>", "**", "//", "<<", ">>", ">=", "==", "<=", "!=", "|", "&", "^", "@", " is not ", " not in ",
    " and ", " or ", " is ", " in ", "+", "-", "*", "/", "%", "<", ">",
];

fn normalized_expression(
    _semantic: &SemanticCallable<'_>,
    expression: &ExpressionAst,
) -> NormalizedExpression {
    let text = expression.text.as_str();
    let mut normalized = NormalizedExpression {
        canonical: Some(text.to_string()),
        ..NormalizedExpression::default()
    };
    match expression.kind {
        AstExpressionKind::Identifier => normalized.identifier = Some(text.to_string()),
        AstExpressionKind::FieldAccess => {
            normalized.member = text.rsplit('.').next().map(str::to_string);
        }
        AstExpressionKind::Assignment => normalized.operator = Some("=".to_string()),
        AstExpressionKind::Conditional => normalized.operator = Some("if-else".to_string()),
        AstExpressionKind::Await => normalized.operator = Some("await".to_string()),
        AstExpressionKind::Yield => normalized.operator = Some("yield".to_string()),
        AstExpressionKind::BinaryOperator => {
            normalized.operator = BINARY_OPERATORS
                .iter()
                .find(|operator| text.contains(**operator))
                .map(|operator| operator.trim().to_string());
        }
        AstExpressionKind::UnaryOperator => {
            normalized.operator = if text.starts_with("not ") {
                Some("not".to_string())
            } else {
                text.chars()
                    .next()
                    .filter(|first| matches!(first, '-' | '+' | '~' | '!'))
                    .map(|first| first.to_string())
            };
        }
        AstExpressionKind::Literal => normalized.literal = Some(literal_value(text)),
        AstExpressionKind::Call | AstExpressionKind::IndexAccess | AstExpressionKind::Unknown => {}
    }
    normalized
}

fn literal_value(text: &str) -> ValueLiteral {
    let trimmed = text.trim();
    match trimmed {
        "None" | "null" | "undefined" => return ValueLiteral::Null,
        "True" | "true" => return ValueLiteral::Boolean(true),
        "False" | "false" => return ValueLiteral::Boolean(false),
        _ => {}
    }
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '_') {
        return ValueLiteral::Integer(trimmed.to_string());
    }
    if !digits.is_empty()
        && digits.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.')
        && digits.parse::<f64>().is_ok()
    {
        return ValueLiteral::Float(trimmed.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix('b').or_else(|| trimmed.strip_prefix('B')) {
        if let Some(inner) = quoted_contents(rest) {
            return ValueLiteral::Bytes(inner.to_string());
        }
    }
    if let Some(inner) = quoted_contents(trimmed) {
        return ValueLiteral::String(inner.to_string());
    }
    ValueLiteral::String(trimmed.to_string())
}

fn quoted_contents(text: &str) -> Option<&str> {
    let mut chars = text.chars();
    let (first, last) = (chars.next()?, chars.next_back()?);
    (matches!(first, '"' | '\'' | '`') && first == last).then(|| &text[1..text.len() - 1])
}

#[cfg(test)]
mod tests {
    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, ExpressionAst, ExpressionKind as AstExpressionKind, FieldAccessAst, FileAst,
        IndexAccessAst, ProjectAst, SourceSpan, StatementAst, StatementKind as AstStatementKind,
    };
    use crate::supergraph::{
        EdgeKind, ExpressionKind, NodeFact, NodeKind, StatementKind, Uncertainty, ValueLiteral,
    };

    #[test]
    fn sg021_lowers_python_expression_facts_and_statement_links() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));
        assert_expression_coverage(&graph, ExpressionKind::Yield);
        assert_statement_links(&graph);
    }

    #[test]
    fn sg021_lowers_typescript_expression_facts_and_statement_links() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));
        assert_expression_coverage(&graph, ExpressionKind::Await);
        assert_statement_links(&graph);
    }

    fn assert_expression_coverage(
        graph: &crate::supergraph::ProgramSupergraph,
        supported_async_kind: ExpressionKind,
    ) {
        let expressions = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Expression(expression) => Some((node, expression)),
                _ => None,
            })
            .collect::<Vec<_>>();

        for kind in [
            ExpressionKind::Identifier,
            ExpressionKind::Literal,
            ExpressionKind::Call,
            ExpressionKind::FieldAccess,
            ExpressionKind::IndexAccess,
            ExpressionKind::Assignment,
            ExpressionKind::BinaryOperator,
            ExpressionKind::UnaryOperator,
            ExpressionKind::Conditional,
            supported_async_kind,
            ExpressionKind::Unknown,
        ] {
            assert!(
                expressions
                    .iter()
                    .any(|(_, expression)| expression.kind == kind),
                "missing expression kind {kind:?}"
            );
        }

        let call = expression_with_text(&expressions, "send(value, obj.field, items[index], 42)");
        assert_eq!(call.kind, ExpressionKind::Call);
        assert!(call.child_expression_ids.len() >= 4);

        let field = expression_with_text(&expressions, "obj.field");
        assert_eq!(field.normalized.member.as_deref(), Some("field"));

        let index = expression_with_text(&expressions, "items[index]");
        assert_eq!(index.kind, ExpressionKind::IndexAccess);
        assert_eq!(index.child_expression_ids.len(), 2);

        let literal = expression_with_text(&expressions, "42");
        assert_eq!(
            literal.normalized.literal,
            Some(ValueLiteral::Integer("42".to_string()))
        );

        let binary = expression_with_text(&expressions, "value + 1");
        assert_eq!(binary.normalized.operator.as_deref(), Some("+"));

        let assignment = expression_with_text(&expressions, "result = value + 1");
        assert_eq!(assignment.normalized.operator.as_deref(), Some("="));

        let unknown = expressions
            .iter()
            .find(|(_, expression)| expression.kind == ExpressionKind::Unknown)
            .expect("unknown expression")
            .0;
        assert_eq!(unknown.uncertainty, Uncertainty::Possible);

        for (node, expression) in &expressions {
            assert_eq!(node.kind, NodeKind::Expression);
            assert!(expression.statement_id.is_some());
            assert!(!node.fact_id.is_empty());
            assert!(!node.payload_hash.is_empty());
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
            .collect::<Vec<_>>();
        assert!(expressions.iter().all(|(_, expression)| {
            contains_edges
                .iter()
                .any(|edge| edge.target_id.as_deref() == Some(expression.expression_id.as_str()))
        }));
    }

    fn assert_statement_links(graph: &crate::supergraph::ProgramSupergraph) {
        let statements = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Statement(statement) => Some(statement),
                _ => None,
            })
            .collect::<Vec<_>>();
        let assignment = statements
            .iter()
            .find(|statement| statement.kind == StatementKind::Assignment)
            .expect("assignment statement");
        assert!(
            !assignment.expression_ids.is_empty(),
            "assignment statement should link root expression"
        );

        let branch = statements
            .iter()
            .find(|statement| statement.kind == StatementKind::Branch)
            .expect("branch statement");
        assert!(
            !branch.expression_ids.is_empty(),
            "branch statement should link condition expression"
        );
    }

    fn expression_with_text<'a>(
        expressions: &'a [(
            &'a crate::supergraph::GraphNode,
            &'a crate::supergraph::Expression,
        )],
        text: &str,
    ) -> &'a crate::supergraph::Expression {
        expressions
            .iter()
            .find(|(_, expression)| expression.original_text.as_deref() == Some(text))
            .map(|(_, expression)| *expression)
            .unwrap_or_else(|| panic!("missing expression text {text}"))
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
                expressions: expressions(owner_id, python),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: vec![FieldAccessAst {
                    object: Some("obj".to_string()),
                    field: "field".to_string(),
                    text: "obj.field".to_string(),
                    owner_id: owner_id.to_string(),
                    source_span: span(41, 50),
                }],
                index_accesses: vec![IndexAccessAst {
                    object: Some("items".to_string()),
                    index: Some("index".to_string()),
                    text: "items[index]".to_string(),
                    owner_id: owner_id.to_string(),
                    source_span: span(52, 64),
                }],
            }],
        }
    }

    fn statements(owner_id: &str, python: bool) -> Vec<StatementAst> {
        vec![
            statement(
                AstStatementKind::Assignment,
                "result = value + 1",
                owner_id,
                span(0, 18),
            ),
            statement(
                AstStatementKind::Expression,
                "send(value, obj.field, items[index], 42)",
                owner_id,
                span(20, 69),
            ),
            statement(
                AstStatementKind::If,
                "if result > 0",
                owner_id,
                span(70, 120),
            ),
            statement(
                if python {
                    AstStatementKind::Expression
                } else {
                    AstStatementKind::Return
                },
                if python {
                    "yield result"
                } else {
                    "return await result"
                },
                owner_id,
                span(121, 140),
            ),
        ]
    }

    fn expressions(owner_id: &str, python: bool) -> Vec<ExpressionAst> {
        vec![
            expression(
                AstExpressionKind::Assignment,
                "result = value + 1",
                owner_id,
                span(0, 18),
            ),
            expression(
                AstExpressionKind::Identifier,
                "result",
                owner_id,
                span(0, 6),
            ),
            expression(
                AstExpressionKind::BinaryOperator,
                "value + 1",
                owner_id,
                span(9, 18),
            ),
            expression(
                AstExpressionKind::Identifier,
                "value",
                owner_id,
                span(9, 14),
            ),
            expression(AstExpressionKind::Literal, "1", owner_id, span(17, 18)),
            expression(
                AstExpressionKind::Call,
                "send(value, obj.field, items[index], 42)",
                owner_id,
                span(20, 69),
            ),
            expression(
                AstExpressionKind::Identifier,
                "send",
                owner_id,
                span(20, 24),
            ),
            expression(
                AstExpressionKind::Identifier,
                "value",
                owner_id,
                span(25, 30),
            ),
            expression(
                AstExpressionKind::FieldAccess,
                "obj.field",
                owner_id,
                span(41, 50),
            ),
            expression(AstExpressionKind::Identifier, "obj", owner_id, span(41, 44)),
            expression(
                AstExpressionKind::Identifier,
                "field",
                owner_id,
                span(45, 50),
            ),
            expression(
                AstExpressionKind::IndexAccess,
                "items[index]",
                owner_id,
                span(52, 64),
            ),
            expression(
                AstExpressionKind::Identifier,
                "items",
                owner_id,
                span(52, 57),
            ),
            expression(
                AstExpressionKind::Identifier,
                "index",
                owner_id,
                span(58, 63),
            ),
            expression(AstExpressionKind::Literal, "42", owner_id, span(66, 68)),
            expression(
                AstExpressionKind::BinaryOperator,
                "result > 0",
                owner_id,
                span(73, 83),
            ),
            expression(
                AstExpressionKind::Identifier,
                "result",
                owner_id,
                span(73, 79),
            ),
            expression(AstExpressionKind::Literal, "0", owner_id, span(82, 83)),
            expression(
                AstExpressionKind::Conditional,
                if python {
                    "a if result else b"
                } else {
                    "result ? a : b"
                },
                owner_id,
                span(84, 105),
            ),
            expression(
                AstExpressionKind::UnaryOperator,
                "not result",
                owner_id,
                span(106, 116),
            ),
            expression(
                if python {
                    AstExpressionKind::Yield
                } else {
                    AstExpressionKind::Await
                },
                if python {
                    "yield result"
                } else {
                    "await result"
                },
                owner_id,
                span(121, 133),
            ),
            expression(
                AstExpressionKind::Identifier,
                "result",
                owner_id,
                span(127, 133),
            ),
            expression(
                AstExpressionKind::Unknown,
                "{unsupported}",
                owner_id,
                span(134, 139),
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

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start,
            end_byte: end,
            start_row: start,
            start_column: 0,
            end_row: end,
            end_column: 0,
        }
    }
}
