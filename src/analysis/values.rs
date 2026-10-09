use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{CallAst, DefinitionKind as AstDefinitionKind, RaiseAst, ReturnAst, SourceSpan};
use crate::supergraph::ids::Tag;
use crate::supergraph::{
    self as sg, Confidence, ExpressionKind, NodeFact, NodeId, NodeKind, ProgramSupergraph,
    ValueKind, ValueLiteral, ValueRole, stable_id,
};

use super::{
    SemanticCallable, SemanticContext, callable_index::CallableIndex, graph_node,
    inference_evidence, insert_node, node_owner, span_contains, span_key,
};

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let mut index = CallableIndex::build(graph);
    for semantic in context.semantic_callables() {
        emit_callable(graph, &mut index, context, &semantic);
    }
}

fn emit_callable(
    graph: &mut ProgramSupergraph,
    index: &mut CallableIndex,
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
) {
    let mut expression_value_ids = BTreeMap::new();

    emit_parameter_values(graph, index, semantic);
    emit_return_values(graph, index, semantic);
    emit_exception_values(graph, index, semantic);
    emit_call_values(graph, index, context, semantic, &mut expression_value_ids);
    emit_literal_values(graph, index, semantic, &mut expression_value_ids);
    emit_expression_values(graph, index, semantic, &mut expression_value_ids);

    index.sync(graph);
    apply_expression_value_ids(graph, index, expression_value_ids);
    apply_definition_value_ids(graph, index, semantic);
    apply_use_value_ids(graph, index, semantic);
}

fn emit_parameter_values(graph: &mut ProgramSupergraph, index: &CallableIndex, semantic: &SemanticCallable<'_>) {
    let callable = semantic.callable();
    let parameters = callable
        .signature
        .parameters
        .iter()
        .filter_map(|parameter| parameter_signature(parameter))
        .collect::<Vec<_>>();
    let receiver_index = receiver_parameter_index(callable.kind, &parameters);

    for (ordinal, parameter) in parameters.iter().enumerate() {
        let span = parameter_binding_span(index, callable.scope_id, &parameter.name)
            .unwrap_or(callable.declaration_span);
        let value_id = value_id(
            callable.callable_id,
            "formal-parameter",
            &parameter.name,
            Some(ordinal),
            Some(span),
        );
        insert_node(
            graph,
            value_node(
                semantic,
                value_id,
                ValueKind::Parameter,
                ValueRole::FormalParameter,
                Some(span),
                None,
                None,
                Some(parameter.name.clone()),
                Some(ordinal),
                None,
                parameter.type_hint.clone(),
                None,
                "callable signature parameter lowered to value",
            ),
        );
    }

    if matches!(
        callable.kind,
        sg::CallableKind::Method | sg::CallableKind::Constructor
    ) {
        let receiver_name = receiver_index
            .and_then(|index| {
                parameters
                    .get(index)
                    .map(|parameter| parameter.name.clone())
            })
            .unwrap_or_else(|| "this".to_string());
        let receiver_ordinal = receiver_index.unwrap_or(0);
        let span = receiver_index
            .and_then(|index| parameters.get(index))
            .and_then(|parameter| {
                parameter_binding_span(index, callable.scope_id, &parameter.name)
            })
            .unwrap_or(callable.declaration_span);
        let value_id = value_id(
            callable.callable_id,
            "receiver",
            &receiver_name,
            Some(receiver_ordinal),
            Some(span),
        );
        insert_node(
            graph,
            value_node(
                semantic,
                value_id,
                ValueKind::Parameter,
                ValueRole::Receiver,
                Some(span),
                None,
                None,
                Some(receiver_name),
                Some(receiver_ordinal),
                None,
                None,
                None,
                "method receiver lowered to value",
            ),
        );
    }
}

fn emit_return_values(graph: &mut ProgramSupergraph, index: &CallableIndex, semantic: &SemanticCallable<'_>) {
    for return_fact in semantic
        .returns()
        .iter()
        .filter(|return_fact| return_fact.owner_id == semantic.owner_id())
    {
        let expression_id = expression_id_for_return(graph, index, semantic, return_fact);
        let value_id = value_id(
            semantic.callable().callable_id,
            "return",
            return_fact.value.as_deref().unwrap_or("return"),
            None,
            Some(return_fact.source_span),
        );
        insert_node(
            graph,
            value_node(
                semantic,
                value_id,
                ValueKind::Return,
                ValueRole::ReturnValue,
                Some(return_fact.source_span),
                expression_id,
                None,
                Some(
                    return_fact
                        .value
                        .clone()
                        .unwrap_or_else(|| "return".to_string()),
                ),
                None,
                None,
                semantic.callable().signature.return_annotation.clone(),
                None,
                "return value lowered to value",
            ),
        );
    }
}

fn emit_exception_values(graph: &mut ProgramSupergraph, index: &CallableIndex, semantic: &SemanticCallable<'_>) {
    for raise in semantic
        .raises()
        .iter()
        .filter(|raise| raise.owner_id == semantic.owner_id())
    {
        let expression_id = expression_id_for_raise(graph, index, semantic, raise);
        let value_id = value_id(
            semantic.callable().callable_id,
            "exception",
            raise.value.as_deref().unwrap_or(&raise.text),
            None,
            Some(raise.source_span),
        );
        insert_node(
            graph,
            value_node(
                semantic,
                value_id,
                ValueKind::Exception,
                ValueRole::ExceptionalValue,
                Some(raise.source_span),
                expression_id,
                None,
                Some(raise.value.clone().unwrap_or_else(|| raise.text.clone())),
                None,
                None,
                None,
                None,
                "raise/throw value lowered to value",
            ),
        );
    }
}

fn emit_call_values(
    graph: &mut ProgramSupergraph, index: &CallableIndex,
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
    expression_value_ids: &mut BTreeMap<NodeId, NodeId>,
) {
    for call in semantic
        .calls()
        .iter()
        .filter(|call| call.context != crate::ast::CallContext::Decorator)
    {
        let Some(call_site) = context.call_site_for(semantic.callable().callable_id, call) else {
            continue;
        };
        let call_expression_id = expression_id_for_call(graph, index, semantic, call);
        let result_id = value_id(
            semantic.callable().callable_id,
            "call-result",
            call_site.call_site_id,
            None,
            Some(call.source_span),
        );
        if let Some(expression_id) = &call_expression_id {
            expression_value_ids.insert(expression_id.clone(), result_id.clone());
        }
        insert_node(
            graph,
            value_node(
                semantic,
                result_id,
                ValueKind::CallResult,
                ValueRole::CallResult,
                Some(call.source_span),
                call_expression_id.clone(),
                Some(call_site.call_site_id.clone()),
                Some(call.callee.clone()),
                None,
                None,
                None,
                None,
                "call result lowered to value",
            ),
        );

        let exception_id = value_id(
            semantic.callable().callable_id,
            "call-exception",
            call_site.call_site_id,
            None,
            Some(call.source_span),
        );
        insert_node(
            graph,
            value_node(
                semantic,
                exception_id,
                ValueKind::Exception,
                ValueRole::ExceptionalValue,
                Some(call.source_span),
                call_expression_id.clone(),
                Some(call_site.call_site_id.clone()),
                Some(call.callee.clone()),
                None,
                None,
                None,
                None,
                "call exceptional result lowered to value",
            ),
        );

        if let Some(receiver) = &call.receiver {
            let receiver_id = value_id(
                semantic.callable().callable_id,
                "receiver-argument",
                call_site.call_site_id,
                Some(0),
                Some(call.source_span),
            );
            insert_node(
                graph,
                value_node(
                    semantic,
                    receiver_id.clone(),
                    ValueKind::Argument,
                    ValueRole::Receiver,
                    Some(call.source_span),
                    receiver_expression_id(graph, index, semantic, call),
                    Some(call_site.call_site_id.clone()),
                    Some(receiver.clone()),
                    Some(0),
                    None,
                    None,
                    None,
                    "call receiver argument lowered to value",
                ),
            );
            emit_mutable_state_value(
                graph,
                semantic,
                call_site,
                call.source_span,
                receiver_id,
                Some(receiver.clone()),
                Some(0),
            );
        }

        for argument in call_arguments(graph, index, call, call_expression_id) {
            let argument_id = value_id(
                semantic.callable().callable_id,
                "argument",
                &format!(
                    "{}:{}",
                    call_site.call_site_id,
                    argument
                        .name
                        .as_deref()
                        .unwrap_or(&argument.ordinal.to_string())
                ),
                Some(argument.ordinal),
                argument.span,
            );
            if let Some(expression_id) = &argument.expression_id {
                expression_value_ids.insert(expression_id.clone(), argument_id.clone());
            }
            insert_node(
                graph,
                value_node(
                    semantic,
                    argument_id.clone(),
                    ValueKind::Argument,
                    ValueRole::Argument,
                    argument.span,
                    argument.expression_id,
                    Some(call_site.call_site_id.clone()),
                    argument.name.clone(),
                    Some(argument.ordinal),
                    None,
                    None,
                    argument.literal,
                    "call argument lowered to value",
                ),
            );
            emit_mutable_state_value(
                graph,
                semantic,
                call_site,
                argument.span.unwrap_or(call.source_span),
                argument_id,
                argument.name,
                Some(argument.ordinal),
            );
        }
    }
}

fn emit_mutable_state_value(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    call_site: &sg::CallSite,
    span: SourceSpan,
    state_of_value_id: NodeId,
    name: Option<String>,
    ordinal: Option<usize>,
) {
    let state_id = value_id(
        semantic.callable().callable_id,
        "mutable-argument-state",
        &format!("{}:{state_of_value_id}", call_site.call_site_id),
        ordinal,
        Some(span),
    );
    insert_node(
        graph,
        value_node(
            semantic,
            state_id,
            ValueKind::Argument,
            ValueRole::MutableArgumentState,
            Some(span),
            None,
            Some(call_site.call_site_id.clone()),
            name,
            ordinal,
            Some(state_of_value_id),
            None,
            None,
            "caller-visible mutable argument state lowered to value",
        ),
    );
}

fn emit_literal_values(
    graph: &mut ProgramSupergraph, index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    expression_value_ids: &mut BTreeMap<NodeId, NodeId>,
) {
    let expressions = expressions_for_callable(graph, index, semantic.callable().callable_id);
    for expression in expressions
        .into_iter()
        .filter(|expression| expression.kind == ExpressionKind::Literal)
    {
        if expression_value_ids.contains_key(&expression.expression_id) {
            continue;
        }
        let span = expression_span(index, expression.expression_id);
        let literal = expression.normalized.literal.clone();
        let value_id = value_id(
            semantic.callable().callable_id,
            "literal",
            expression.original_text.as_deref().unwrap_or("literal"),
            None,
            span,
        );
        expression_value_ids.insert(expression.expression_id.clone(), value_id.clone());
        insert_node(
            graph,
            value_node(
                semantic,
                value_id,
                ValueKind::Literal,
                ValueRole::Unknown,
                span,
                Some(expression.expression_id),
                None,
                None,
                None,
                None,
                None,
                literal,
                "literal expression lowered to value",
            ),
        );
    }
}

fn emit_expression_values(
    graph: &mut ProgramSupergraph, index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    expression_value_ids: &mut BTreeMap<NodeId, NodeId>,
) {
    let expressions = expressions_for_callable(graph, index, semantic.callable().callable_id);
    for expression in expressions {
        if expression_value_ids.contains_key(&expression.expression_id) {
            continue;
        }
        let Some(span) = expression_span(index, expression.expression_id) else {
            continue;
        };
        let (kind, confidence, evidence) = expression_value_shape(&expression);
        let value_id = value_id(
            semantic.callable().callable_id,
            &format!("expression-{}", expression_kind_key(expression.kind)),
            expression.expression_id,
            Some(expression.ordinal),
            Some(span),
        );
        expression_value_ids.insert(expression.expression_id.clone(), value_id.clone());
        insert_node(
            graph,
            value_node(
                semantic,
                value_id,
                kind,
                ValueRole::Unknown,
                Some(span),
                Some(expression.expression_id.clone()),
                None,
                expression_value_name(&expression),
                Some(expression.ordinal),
                None,
                None,
                expression.normalized.literal,
                evidence,
            )
            .with_confidence(confidence),
        );
    }
}

fn apply_expression_value_ids(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    expression_value_ids: BTreeMap<NodeId, NodeId>,
) {
    for expression_id in expression_value_ids.keys() {
        let Some(position) = index.expression_position(*expression_id) else {
            continue;
        };
        if let NodeFact::Expression(expression) = &mut graph.nodes[position].fact {
            expression.value_id = expression_value_ids.get(expression_id).cloned();
        }
    }
}

fn apply_definition_value_ids(graph: &mut ProgramSupergraph, index: &CallableIndex, semantic: &SemanticCallable<'_>) {
    let callable_id = semantic.callable().callable_id.clone();
    let parameter_values = values_for_callable(graph, index, callable_id)
        .into_iter()
        .filter(|value| value.role == ValueRole::FormalParameter)
        .filter_map(|value| {
            value
                .name
                .clone()
                .map(|name| (name, value.value_id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let expressions = expressions_for_callable(graph, index, callable_id);
    let expression_values_by_span = expressions
        .iter()
        .filter_map(|expression| {
            Some((
                expression_span(index, expression.expression_id)?,
                expression.clone(),
            ))
        })
        .collect::<Vec<_>>();

    for definition in semantic
        .definitions()
        .iter()
        .filter(|definition| definition.owner_id == semantic.owner_id())
    {
        let value_id = if definition.kind == AstDefinitionKind::Parameter {
            parameter_values.get(&definition.name).cloned()
        } else {
            expression_values_by_span
                .iter()
                .find(|(span, expression)| {
                    *span == definition.source_span
                        && matches!(
                            expression.kind,
                            ExpressionKind::FieldAccess | ExpressionKind::IndexAccess
                        )
                })
                .and_then(|(_, expression)| expression.value_id.clone())
                .or_else(|| {
                    expression_values_by_span
                        .iter()
                        .filter(|(span, expression)| {
                            span_contains(*span, definition.source_span)
                                && expression.kind == ExpressionKind::Assignment
                        })
                        .min_by_key(|(span, _)| span.end_byte.saturating_sub(span.start_byte))
                        .and_then(|(_, expression)| expression.value_id.clone())
                })
                .or_else(|| {
                    expression_values_by_span
                        .iter()
                        .find(|(span, _)| *span == definition.source_span)
                        .and_then(|(_, expression)| expression.value_id.clone())
                })
        };
        let Some(value_id) = value_id else {
            continue;
        };
        for &position in index.positions(callable_id) {
            let node = &mut graph.nodes[position];
            let NodeFact::Definition(existing) = &mut node.fact else {
                continue;
            };
            if existing.name.as_deref() == Some(definition.name.as_str())
                && node.span == Some(definition.source_span)
            {
                existing.value_id = Some(value_id.clone());
            }
        }
    }
}

fn apply_use_value_ids(graph: &mut ProgramSupergraph, index: &CallableIndex, semantic: &SemanticCallable<'_>) {
    let callable_id = semantic.callable().callable_id.clone();
    let values_by_expression_span = expressions_for_callable(graph, index, callable_id)
        .into_iter()
        .filter_map(|expression| {
            Some((
                expression_span(index, expression.expression_id)?,
                expression.value_id.clone()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();

    for use_fact in semantic
        .uses()
        .iter()
        .filter(|use_fact| use_fact.owner_id == semantic.owner_id())
    {
        let Some(value_id) = values_by_expression_span
            .get(&use_fact.source_span)
            .cloned()
        else {
            continue;
        };
        for &position in index.positions(callable_id) {
            let node = &mut graph.nodes[position];
            let NodeFact::Use(existing) = &mut node.fact else {
                continue;
            };
            if existing.name.as_deref() == Some(use_fact.name.as_str())
                && node.span == Some(use_fact.source_span)
            {
                existing.value_id = Some(value_id.clone());
            }
        }
    }
}

fn value_node(
    semantic: &SemanticCallable<'_>,
    value_id: NodeId,
    kind: ValueKind,
    role: ValueRole,
    span: Option<SourceSpan>,
    expression_id: Option<NodeId>,
    call_site_id: Option<NodeId>,
    name: Option<String>,
    ordinal: Option<usize>,
    state_of_value_id: Option<NodeId>,
    type_hint: Option<String>,
    literal: Option<ValueLiteral>,
    evidence: &str,
) -> sg::GraphNode {
    graph_node(
        value_id.clone(),
        NodeKind::Value,
        node_owner(semantic),
        span,
        Confidence::Exact,
        inference_evidence(evidence),
        NodeFact::Value(sg::Value {
            value_id,
            callable_id: Some(semantic.callable().callable_id.clone()),
            kind,
            role,
            symbol_id: None,
            expression_id,
            call_site_id,
            name,
            ordinal,
            state_of_value_id,
            type_hint,
            literal,
        }),
    )
}

trait GraphNodeConfidenceExt {
    fn with_confidence(self, confidence: Confidence) -> sg::GraphNode;
}

impl GraphNodeConfidenceExt for sg::GraphNode {
    fn with_confidence(mut self, confidence: Confidence) -> sg::GraphNode {
        self.confidence = confidence;
        self.uncertainty = sg::uncertainty_from_confidence(&confidence);
        self
    }
}

fn expression_value_shape(expression: &sg::Expression) -> (ValueKind, Confidence, &'static str) {
    match expression.kind {
        ExpressionKind::Literal => (
            ValueKind::Literal,
            Confidence::Exact,
            "literal expression lowered to value",
        ),
        ExpressionKind::FieldAccess => (
            ValueKind::Field,
            Confidence::Unknown,
            "field access expression lowered to uncertain value",
        ),
        ExpressionKind::IndexAccess => (
            ValueKind::Index,
            Confidence::Unknown,
            "index access expression lowered to uncertain value",
        ),
        ExpressionKind::Identifier => (
            ValueKind::Unknown,
            Confidence::Probable,
            "identifier expression lowered to value instance",
        ),
        ExpressionKind::Unknown => (
            ValueKind::Unknown,
            Confidence::Unknown,
            "unknown expression retained as uncertain value instance",
        ),
        _ => (
            ValueKind::ComputedExpression,
            Confidence::Exact,
            "computed expression lowered to value instance",
        ),
    }
}

fn expression_value_name(expression: &sg::Expression) -> Option<String> {
    expression
        .normalized
        .identifier
        .clone()
        .or_else(|| expression.normalized.member.clone())
        .or_else(|| expression.original_text.clone())
}

fn expression_kind_key(kind: ExpressionKind) -> &'static str {
    match kind {
        ExpressionKind::Identifier => "identifier",
        ExpressionKind::Literal => "literal",
        ExpressionKind::Call => "call",
        ExpressionKind::FieldAccess => "field-access",
        ExpressionKind::IndexAccess => "index-access",
        ExpressionKind::Assignment => "assignment",
        ExpressionKind::UnaryOperator => "unary-operator",
        ExpressionKind::BinaryOperator => "binary-operator",
        ExpressionKind::Conditional => "conditional",
        ExpressionKind::Await => "await",
        ExpressionKind::Yield => "yield",
        ExpressionKind::Lambda => "lambda",
        ExpressionKind::Unknown => "unknown",
    }
}

fn value_id<'a>(
    callable_id: NodeId,
    role_key: &str,
    semantic_key: impl Into<crate::supergraph::ids::IdPart<'a>>,
    ordinal: Option<usize>,
    span: Option<SourceSpan>,
) -> NodeId {
    stable_id(
        Tag::Value,
        crate::id_parts![
            callable_id,
            role_key,
            semantic_key.into(),
            &ordinal
                .map(|ordinal| ordinal.to_string())
                .unwrap_or_default(),
            &span.map(span_key).unwrap_or_default(),
        ],
    )
}

fn parameter_binding_span(
    index: &CallableIndex,
    scope_id: NodeId,
    parameter_name: &str,
) -> Option<SourceSpan> {
    index.parameter_binding_span(scope_id, parameter_name)
}

fn parameter_signature(parameter: &str) -> Option<ParameterSignature> {
    let trimmed = parameter.trim();
    let without_default = trimmed.split('=').next().unwrap_or(trimmed).trim();
    let mut parts = without_default.splitn(2, ':');
    let name_part = parts.next().unwrap_or(without_default).trim();
    let type_hint = parts.next().map(str::trim).filter(|hint| !hint.is_empty());
    let name = name_part
        .trim_end_matches('?')
        .trim_start_matches("...")
        .trim_start_matches('*')
        .split_whitespace()
        .last()
        .unwrap_or(name_part)
        .trim()
        .to_string();
    (!name.is_empty()).then(|| ParameterSignature {
        name,
        type_hint: type_hint.map(str::to_string),
    })
}

fn receiver_parameter_index(
    callable_kind: sg::CallableKind,
    parameters: &[ParameterSignature],
) -> Option<usize> {
    if !matches!(
        callable_kind,
        sg::CallableKind::Method | sg::CallableKind::Constructor
    ) {
        return None;
    }
    parameters
        .iter()
        .position(|parameter| matches!(parameter.name.as_str(), "self" | "this" | "cls"))
}

fn expression_id_for_return(
    graph: &ProgramSupergraph, index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    return_fact: &ReturnAst,
) -> Option<NodeId> {
    expression_id_for_text_in_span(
        graph,
        index,
        semantic.callable().callable_id,
        return_fact.value.as_deref(),
        return_fact.source_span,
    )
}

fn expression_id_for_raise(
    graph: &ProgramSupergraph, index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    raise: &RaiseAst,
) -> Option<NodeId> {
    expression_id_for_text_in_span(
        graph,
        index,
        semantic.callable().callable_id,
        raise.value.as_deref(),
        raise.source_span,
    )
}

fn expression_id_for_call(
    graph: &ProgramSupergraph, index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    call: &CallAst,
) -> Option<NodeId> {
    index
        .nodes(graph, semantic.callable().callable_id)
        .find_map(|node| match &node.fact {
            NodeFact::Expression(expression)
                if expression.kind == ExpressionKind::Call
                    && node.span == Some(call.source_span) =>
            {
                Some(expression.expression_id.clone())
            }
            _ => None,
        })
}

fn receiver_expression_id(
    graph: &ProgramSupergraph, index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    call: &CallAst,
) -> Option<NodeId> {
    expression_id_for_text_in_span(
        graph,
        index,
        semantic.callable().callable_id,
        call.receiver.as_deref(),
        call.source_span,
    )
}

fn expression_id_for_text_in_span(
    graph: &ProgramSupergraph, index: &CallableIndex,
    callable_id: NodeId,
    text: Option<&str>,
    enclosing_span: SourceSpan,
) -> Option<NodeId> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Expression(expression) => Some((node, expression)),
            _ => None,
        })
        .filter(|(node, expression)| {
            node.span
                .is_some_and(|span| span_contains(enclosing_span, span))
                && text.is_none_or(|text| expression.original_text.as_deref() == Some(text))
        })
        .min_by_key(|(node, _)| {
            node.span
                .map(|span| span.end_byte.saturating_sub(span.start_byte))
                .unwrap_or(usize::MAX)
        })
        .map(|(_, expression)| expression.expression_id.clone())
}

fn call_arguments(
    graph: &ProgramSupergraph, index: &CallableIndex,
    call: &CallAst,
    call_expression_id: Option<NodeId>,
) -> Vec<ArgumentValue> {
    let mut direct_children = call_expression_id
        .and_then(|expression_id| expression_by_id(graph, index, expression_id))
        .map(|call_expression| {
            call_expression
                .child_expression_ids
                .iter()
                .filter_map(|child_id| expression_by_id(graph, index, *child_id))
                .filter(|expression| {
                    expression.original_text.as_deref() != Some(call.callee.as_str())
                })
                .filter(|expression| {
                    call.receiver.as_ref().is_none_or(|receiver| {
                        expression.original_text.as_deref() != Some(receiver.as_str())
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    direct_children.sort_by_key(|expression| {
        expression_span(index, expression.expression_id).unwrap_or(call.source_span)
    });

    let total_count = call.args_count;
    let named_count = call.argument_names.len();
    let positional_count = total_count.saturating_sub(named_count);
    let names_by_ordinal = (0..positional_count)
        .map(|ordinal| (ordinal, None))
        .chain(
            call.argument_names
                .iter()
                .enumerate()
                .map(|(index, name)| (positional_count + index, Some(name.clone()))),
        )
        .collect::<BTreeMap<_, _>>();
    let mut used = BTreeSet::new();
    let mut arguments = Vec::new();

    for ordinal in 0..total_count {
        let expression = direct_children.get(ordinal).cloned();
        if let Some(expression) = &expression {
            used.insert(expression.expression_id.clone());
        }
        arguments.push(ArgumentValue {
            ordinal,
            name: names_by_ordinal.get(&ordinal).cloned().flatten(),
            expression_id: expression
                .as_ref()
                .map(|expression| expression.expression_id.clone()),
            span: expression
                .as_ref()
                .and_then(|expression| expression_span(index, expression.expression_id))
                .or(Some(call.source_span)),
            literal: expression.and_then(|expression| expression.normalized.literal),
        });
    }

    for expression in direct_children {
        if used.contains(&expression.expression_id) {
            continue;
        }
        let ordinal = arguments.len();
        arguments.push(ArgumentValue {
            ordinal,
            name: None,
            expression_id: Some(expression.expression_id.clone()),
            span: expression_span(index, expression.expression_id),
            literal: expression.normalized.literal,
        });
    }

    arguments
}

fn expressions_for_callable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
) -> Vec<sg::Expression> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Expression(expression) => Some(expression.clone()),
            _ => None,
        })
        .collect()
}

fn expression_by_id(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    expression_id: NodeId,
) -> Option<sg::Expression> {
    let node = &graph.nodes[index.expression_position(expression_id)?];
    match &node.fact {
        NodeFact::Expression(expression) => Some(expression.clone()),
        _ => None,
    }
}

fn values_for_callable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
) -> Vec<sg::Value> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Value(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn expression_span(index: &CallableIndex, expression_id: NodeId) -> Option<SourceSpan> {
    index.expression_span(expression_id)
}

#[derive(Debug)]
struct ParameterSignature {
    name: String,
    type_hint: Option<String>,
}

#[derive(Debug)]
struct ArgumentValue {
    ordinal: usize,
    name: Option<String>,
    expression_id: Option<NodeId>,
    span: Option<SourceSpan>,
    literal: Option<ValueLiteral>,
}

#[cfg(test)]
mod tests {
    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, CallContext, ExpressionAst, ExpressionKind as AstExpressionKind, FileAst,
        ParamAst, ProjectAst, RaiseAst, ReturnAst, SourceSpan, StatementAst,
        StatementKind as AstStatementKind, SymbolAst, SymbolKind,
    };
    use crate::supergraph::{EdgeKind, NodeFact, NodeKind, ValueRole};

    #[test]
    fn sg023_lowers_python_values_and_interprocedural_value_edges() {
        let graph = build_python_supergraph(&python_project());

        assert_value_role(&graph, ValueRole::FormalParameter, Some("items"));
        assert_value_role(&graph, ValueRole::Receiver, Some("self"));
        assert_value_role(&graph, ValueRole::Argument, None);
        assert_value_role(&graph, ValueRole::ReturnValue, Some("item"));
        assert_value_role(&graph, ValueRole::CallResult, Some("process"));
        assert_value_role(&graph, ValueRole::ExceptionalValue, Some("ValueError()"));
        assert_value_role(&graph, ValueRole::MutableArgumentState, None);
        assert_interprocedural_edges_connect_values(&graph);
    }

    #[test]
    fn sg023_lowers_typescript_this_receiver_and_call_values() {
        let graph = build_typescript_supergraph(&typescript_project());

        assert_value_role(&graph, ValueRole::FormalParameter, Some("items"));
        assert_value_role(&graph, ValueRole::Receiver, Some("this"));
        assert_value_role(&graph, ValueRole::Receiver, Some("service"));
        assert_value_role(&graph, ValueRole::Argument, None);
        assert_value_role(&graph, ValueRole::ReturnValue, Some("items"));
        assert_value_role(&graph, ValueRole::CallResult, Some("service.process"));
        assert_value_role(&graph, ValueRole::ExceptionalValue, Some("new Error()"));
        assert_value_role(&graph, ValueRole::MutableArgumentState, None);
    }

    fn assert_value_role(
        graph: &crate::supergraph::ProgramSupergraph,
        expected_role: ValueRole,
        expected_name: Option<&str>,
    ) {
        assert!(
            graph.nodes.iter().any(|node| {
                node.kind == NodeKind::Value
                    && matches!(
                        &node.fact,
                        NodeFact::Value(value)
                            if value.role == expected_role
                                && expected_name.is_none_or(|name| value.name.as_deref() == Some(name))
                    )
            }),
            "missing value role {expected_role:?} with name {expected_name:?}"
        );
    }

    fn assert_interprocedural_edges_connect_values(graph: &crate::supergraph::ProgramSupergraph) {
        for kind in [EdgeKind::ParameterIn, EdgeKind::ReturnsTo] {
            let edge = graph
                .edges
                .iter()
                .find(|edge| edge.kind == kind)
                .unwrap_or_else(|| panic!("missing edge kind {kind:?}"));
            assert_eq!(node_kind(graph, edge.source_id), Some(NodeKind::Value));
            assert_eq!(
                edge.target_id.as_ref().and_then(|id| node_kind(graph, *id)),
                Some(NodeKind::Value),
                "{kind:?} target should be a value"
            );
        }
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::ParameterOut)
        {
            assert_eq!(node_kind(graph, edge.source_id), Some(NodeKind::Value));
            assert_eq!(
                edge.target_id.as_ref().and_then(|id| node_kind(graph, *id)),
                Some(NodeKind::Value),
                "ParameterOut target should be a value when mutation summaries exist"
            );
        }
    }

    fn node_kind(graph: &crate::supergraph::ProgramSupergraph, node_id: crate::supergraph::NodeId) -> Option<NodeKind> {
        graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.kind)
    }

    fn python_project() -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: "sample.py".to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![
                    SymbolAst {
                        id: "sample:Worker.process".to_string(),
                        name: "process".to_string(),
                        kind: SymbolKind::Method,
                        module_path: "sample".to_string(),
                        parent: Some("Worker".to_string()),
                        parameters: vec![
                            param("self", "self", span(12, 16)),
                            param("items", "items: list", span(18, 29)),
                        ],
                        decorators: Vec::new(),
                        return_type: Some("Item".to_string()),
                        body_span: Some(span(40, 110)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: vec![RaiseAst {
                            text: "raise ValueError()".to_string(),
                            value: Some("ValueError()".to_string()),
                            owner_id: "sample:Worker.process".to_string(),
                            source_span: span(70, 88),
                        }],
                        statements: vec![
                            statement(
                                AstStatementKind::Return,
                                "return item",
                                span(50, 61),
                                "sample:Worker.process",
                            ),
                            statement(
                                AstStatementKind::Raise,
                                "raise ValueError()",
                                span(70, 88),
                                "sample:Worker.process",
                            ),
                        ],
                        expressions: vec![
                            expression(
                                AstExpressionKind::Identifier,
                                "item",
                                span(57, 61),
                                "sample:Worker.process",
                            ),
                            expression(
                                AstExpressionKind::Call,
                                "ValueError()",
                                span(76, 88),
                                "sample:Worker.process",
                            ),
                        ],
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: vec![ReturnAst {
                            value: Some("item".to_string()),
                            owner_id: "sample:Worker.process".to_string(),
                            source_span: span(50, 61),
                        }],
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(0, 110),
                    },
                    SymbolAst {
                        id: "sample:process".to_string(),
                        name: "process".to_string(),
                        kind: SymbolKind::Function,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: vec![param("items", "items: list", span(112, 123))],
                        decorators: Vec::new(),
                        return_type: Some("Item".to_string()),
                        body_span: Some(span(124, 180)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: vec![RaiseAst {
                            text: "raise ValueError()".to_string(),
                            value: Some("ValueError()".to_string()),
                            owner_id: "sample:process".to_string(),
                            source_span: span(150, 168),
                        }],
                        statements: vec![
                            statement(
                                AstStatementKind::Return,
                                "return item",
                                span(130, 141),
                                "sample:process",
                            ),
                            statement(
                                AstStatementKind::Raise,
                                "raise ValueError()",
                                span(150, 168),
                                "sample:process",
                            ),
                        ],
                        expressions: vec![
                            expression(
                                AstExpressionKind::Identifier,
                                "item",
                                span(137, 141),
                                "sample:process",
                            ),
                            expression(
                                AstExpressionKind::Call,
                                "ValueError()",
                                span(156, 168),
                                "sample:process",
                            ),
                        ],
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: vec![ReturnAst {
                            value: Some("item".to_string()),
                            owner_id: "sample:process".to_string(),
                            source_span: span(130, 141),
                        }],
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(112, 180),
                    },
                    SymbolAst {
                        id: "sample:caller".to_string(),
                        name: "caller".to_string(),
                        kind: SymbolKind::Function,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: Vec::new(),
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(120, 180)),
                        assignments: Vec::new(),
                        calls: vec![CallAst {
                            callee: "process".to_string(),
                            receiver: None,
                            argument_names: Vec::new(),
                            args_count: 1,
                            context: CallContext::Body,
                            source_span: span(140, 161),
                        }],
                        raises: Vec::new(),
                        statements: vec![statement(
                            AstStatementKind::Return,
                            "return process(items)",
                            span(130, 168),
                            "sample:caller",
                        )],
                        expressions: vec![
                            expression(
                                AstExpressionKind::Call,
                                "process(items)",
                                span(140, 161),
                                "sample:caller",
                            ),
                            expression(
                                AstExpressionKind::Identifier,
                                "process",
                                span(140, 147),
                                "sample:caller",
                            ),
                            expression(
                                AstExpressionKind::Identifier,
                                "items",
                                span(155, 160),
                                "sample:caller",
                            ),
                        ],
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(120, 180),
                    },
                ],
                statements: Vec::new(),
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

    fn typescript_project() -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: "sample.ts".to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![
                    SymbolAst {
                        id: "sample:Service.process".to_string(),
                        name: "process".to_string(),
                        kind: SymbolKind::Method,
                        module_path: "sample".to_string(),
                        parent: Some("Service".to_string()),
                        parameters: vec![param("items", "items: Item[]", span(20, 33))],
                        decorators: Vec::new(),
                        return_type: Some("Item[]".to_string()),
                        body_span: Some(span(40, 110)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: vec![RaiseAst {
                            text: "throw new Error()".to_string(),
                            value: Some("new Error()".to_string()),
                            owner_id: "sample:Service.process".to_string(),
                            source_span: span(75, 92),
                        }],
                        statements: vec![
                            statement(
                                AstStatementKind::Return,
                                "return items",
                                span(50, 62),
                                "sample:Service.process",
                            ),
                            statement(
                                AstStatementKind::Throw,
                                "throw new Error()",
                                span(75, 92),
                                "sample:Service.process",
                            ),
                        ],
                        expressions: vec![
                            expression(
                                AstExpressionKind::Identifier,
                                "items",
                                span(57, 62),
                                "sample:Service.process",
                            ),
                            expression(
                                AstExpressionKind::Call,
                                "new Error()",
                                span(81, 92),
                                "sample:Service.process",
                            ),
                        ],
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: vec![ReturnAst {
                            value: Some("items".to_string()),
                            owner_id: "sample:Service.process".to_string(),
                            source_span: span(50, 62),
                        }],
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(0, 110),
                    },
                    SymbolAst {
                        id: "sample:caller".to_string(),
                        name: "caller".to_string(),
                        kind: SymbolKind::Function,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: Vec::new(),
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(120, 180)),
                        assignments: Vec::new(),
                        calls: vec![CallAst {
                            callee: "service.process".to_string(),
                            receiver: Some("service".to_string()),
                            argument_names: Vec::new(),
                            args_count: 1,
                            context: CallContext::Body,
                            source_span: span(135, 157),
                        }],
                        raises: Vec::new(),
                        statements: vec![statement(
                            AstStatementKind::Return,
                            "return service.process(items)",
                            span(128, 165),
                            "sample:caller",
                        )],
                        expressions: vec![
                            expression(
                                AstExpressionKind::Call,
                                "service.process(items)",
                                span(135, 157),
                                "sample:caller",
                            ),
                            expression(
                                AstExpressionKind::FieldAccess,
                                "service.process",
                                span(135, 150),
                                "sample:caller",
                            ),
                            expression(
                                AstExpressionKind::Identifier,
                                "service",
                                span(135, 142),
                                "sample:caller",
                            ),
                            expression(
                                AstExpressionKind::Identifier,
                                "items",
                                span(151, 156),
                                "sample:caller",
                            ),
                        ],
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(120, 180),
                    },
                ],
                statements: Vec::new(),
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

    fn param(name: &str, text: &str, source_span: SourceSpan) -> ParamAst {
        ParamAst {
            name: name.to_string(),
            text: text.to_string(),
            source_span,
        }
    }

    fn statement(
        kind: AstStatementKind,
        text: &str,
        source_span: SourceSpan,
        owner_id: &str,
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
        source_span: SourceSpan,
        owner_id: &str,
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
