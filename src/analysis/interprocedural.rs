use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ast::SourceSpan;
use crate::supergraph::{
    self as sg, Confidence, ControlFlowNodeRole, ControlRegionKind, DataFlowKind, DiagnosticKind,
    DispatchKind, EdgeFact, EdgeKind, ExpressionKind, NodeFact, NodeId, NodeKind,
    ProgramSupergraph, Resolution, Severity, ValueRole,
};

use super::{
    callable_index::CallableIndex, edge_id, graph_edge, graph_node, inference_evidence, insert_edge,
    insert_node,
};

const SG080_EXACT_PRECISION: &str = "sg080-actual-argument-to-formal-parameter-value";
const SG080_POSSIBLE_PRECISION: &str =
    "sg080-possible-call-target-actual-argument-to-formal-parameter-value";
const SG080_UNAVAILABLE_PRECISION: &str =
    "sg080-formal-parameter-value-unavailable-for-call-target";
const SG081_EXACT_PRECISION: &str = "sg081-receiver-value-to-self-this-formal-value";
const SG081_POSSIBLE_PRECISION: &str =
    "sg081-possible-call-target-receiver-value-to-self-this-formal-value";
const SG082_EXACT_PRECISION: &str = "sg082-callee-return-value-to-call-result-value";
const SG082_POSSIBLE_PRECISION: &str =
    "sg082-possible-call-target-callee-return-value-to-call-result-value";
const SG083_FIELD_PRECISION: &str = "sg083-parameter-out-field-mutation-summary";
const SG083_INDEX_PRECISION: &str = "sg083-parameter-out-index-mutation-summary";
const SG083_ALIAS_PRECISION: &str = "sg083-parameter-out-possible-alias-mutation-summary";
const SG083_POSSIBLE_CALL_FIELD_PRECISION: &str =
    "sg083-possible-call-target-parameter-out-field-mutation-summary";
const SG083_POSSIBLE_CALL_INDEX_PRECISION: &str =
    "sg083-possible-call-target-parameter-out-index-mutation-summary";
const SG083_POSSIBLE_CALL_ALIAS_PRECISION: &str =
    "sg083-possible-call-target-parameter-out-possible-alias-mutation-summary";
const SG084_EXACT_CALL_EXCEPTION_PRECISION: &str =
    "sg084-callee-exception-value-to-call-exception-value";
const SG084_POSSIBLE_CALL_EXCEPTION_PRECISION: &str =
    "sg084-possible-call-target-callee-exception-value-to-call-exception-value";
const SG084_CALLEE_HANDLER_PRECISION: &str = "sg084-callee-handled-exception-value-to-handler";
const SG084_CALLEE_EXCEPTIONAL_EXIT_PRECISION: &str =
    "sg084-callee-unhandled-exception-value-to-exceptional-exit";
const SG084_CALLER_HANDLER_PRECISION: &str =
    "sg084-callee-unhandled-exception-value-to-caller-handler";
const SG084_CALLER_EXCEPTIONAL_EXIT_PRECISION: &str =
    "sg084-callee-unhandled-exception-value-to-caller-exceptional-exit";

pub(crate) fn emit(graph: &mut ProgramSupergraph) {
    let callable_index = CallableIndex::build(graph);
    let mut callables = BTreeMap::new();
    let mut call_sites = BTreeMap::new();
    let mut parameter_values = BTreeMap::<(NodeId, String), NodeId>::new();
    let mut receiver_values = BTreeMap::<NodeId, (NodeId, String, usize)>::new();
    let mut return_values = BTreeMap::<NodeId, Vec<NodeId>>::new();
    let mut exception_values = BTreeMap::<NodeId, Vec<NodeId>>::new();
    let mut call_result_values = BTreeMap::<NodeId, NodeId>::new();
    let mut call_exception_values = BTreeMap::<NodeId, NodeId>::new();
    let mut call_argument_values = BTreeMap::<(NodeId, usize), NodeId>::new();
    let mut named_argument_values = BTreeMap::<(NodeId, String), (NodeId, usize)>::new();
    let mut call_receiver_values = BTreeMap::<NodeId, NodeId>::new();
    let mut mutable_state_values = BTreeMap::<NodeId, NodeId>::new();
    let mut values_by_id = BTreeMap::<NodeId, sg::Value>::new();
    let mut value_spans_by_id = BTreeMap::<NodeId, SourceSpan>::new();
    let mut expressions_by_id = BTreeMap::<NodeId, sg::Expression>::new();

    for node in &graph.nodes {
        match &node.fact {
            NodeFact::Callable(callable) => {
                callables.insert(callable.callable_id.clone(), callable.clone());
            }
            NodeFact::CallSite(call_site) => {
                call_sites.insert(call_site.call_site_id.clone(), call_site.clone());
            }
            NodeFact::Expression(expression) => {
                expressions_by_id.insert(expression.expression_id.clone(), expression.clone());
            }
            NodeFact::Value(value) => {
                values_by_id.insert(value.value_id.clone(), value.clone());
                if let Some(span) = node.span {
                    value_spans_by_id.insert(value.value_id.clone(), span);
                }
                match value.role {
                    ValueRole::FormalParameter => {
                        if let (Some(callable_id), Some(name)) = (&value.callable_id, &value.name) {
                            parameter_values.insert(
                                (callable_id.clone(), name.clone()),
                                value.value_id.clone(),
                            );
                        }
                    }
                    ValueRole::Receiver => {
                        if let Some(call_site_id) = &value.call_site_id {
                            call_receiver_values
                                .insert(call_site_id.clone(), value.value_id.clone());
                        } else if let (Some(callable_id), Some(name)) =
                            (&value.callable_id, &value.name)
                        {
                            receiver_values.insert(
                                callable_id.clone(),
                                (
                                    value.value_id.clone(),
                                    name.clone(),
                                    value.ordinal.unwrap_or_default(),
                                ),
                            );
                        }
                    }
                    ValueRole::Argument => {
                        if let (Some(call_site_id), Some(ordinal)) =
                            (&value.call_site_id, value.ordinal)
                        {
                            call_argument_values
                                .insert((call_site_id.clone(), ordinal), value.value_id.clone());
                            if let Some(name) = &value.name {
                                named_argument_values.insert(
                                    (call_site_id.clone(), name.clone()),
                                    (value.value_id.clone(), ordinal),
                                );
                            }
                        }
                    }
                    ValueRole::ReturnValue => {
                        if let Some(callable_id) = &value.callable_id {
                            return_values
                                .entry(callable_id.clone())
                                .or_default()
                                .push(value.value_id.clone());
                        }
                    }
                    ValueRole::CallResult => {
                        if let Some(call_site_id) = &value.call_site_id {
                            call_result_values.insert(call_site_id.clone(), value.value_id.clone());
                        }
                    }
                    ValueRole::ExceptionalValue => {
                        if let Some(call_site_id) = &value.call_site_id {
                            call_exception_values
                                .insert(call_site_id.clone(), value.value_id.clone());
                        } else if let Some(callable_id) = &value.callable_id {
                            exception_values
                                .entry(callable_id.clone())
                                .or_default()
                                .push(value.value_id.clone());
                        }
                    }
                    ValueRole::MutableArgumentState => {
                        if let Some(state_of_value_id) = &value.state_of_value_id {
                            mutable_state_values
                                .insert(state_of_value_id.clone(), value.value_id.clone());
                        }
                    }
                    ValueRole::Unknown => {}
                }
            }
            _ => {}
        }
    }

    for nodes in return_values.values_mut() {
        nodes.sort();
        nodes.dedup();
    }
    for nodes in exception_values.values_mut() {
        nodes.sort();
        nodes.dedup();
    }

    let call_edges = graph
        .edges
        .iter()
        .filter_map(|edge| {
            let EdgeFact::Calls(calls) = &edge.fact else {
                return None;
            };
            Some((edge.clone(), calls.clone()))
        })
        .collect::<Vec<_>>();
    let parameter_out_summaries = parameter_out_summaries(graph, &values_by_id, &expressions_by_id);

    for (edge, calls) in call_edges {
        let actual_count = actual_argument_count(&call_argument_values, &calls.call_site_id);
        let Some(callee_id) = calls.callee_callable_id.clone() else {
            if let Some(call_site) = call_sites.get(&calls.call_site_id) {
                if actual_count > 0 {
                    add_formal_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        unavailable_target_reason(&calls),
                    );
                }
                if has_receiver_flow_input(call_site, &call_receiver_values, &calls.call_site_id) {
                    add_receiver_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        unavailable_target_reason(&calls),
                    );
                }
                if call_result_values.contains_key(&calls.call_site_id) {
                    add_return_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        unavailable_return_target_reason(&calls),
                    );
                }
                if call_exception_values.contains_key(&calls.call_site_id) {
                    add_throws_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        unavailable_exception_target_reason(&calls),
                    );
                }
            }
            continue;
        };
        let Some(callee) = callables.get(&callee_id) else {
            if let Some(call_site) = call_sites.get(&calls.call_site_id) {
                if actual_count > 0 {
                    add_formal_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        "resolved callee callable is missing from graph values",
                    );
                }
                if has_receiver_flow_input(call_site, &call_receiver_values, &calls.call_site_id) {
                    add_receiver_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        "resolved callee callable is missing from graph values",
                    );
                }
                if call_result_values.contains_key(&calls.call_site_id) {
                    add_return_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        "resolved callee callable is missing from graph values",
                    );
                }
                if call_exception_values.contains_key(&calls.call_site_id) {
                    add_throws_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        "resolved callee callable is missing from graph exception summaries",
                    );
                }
            }
            continue;
        };
        let Some(call_site) = call_sites.get(&calls.call_site_id) else {
            continue;
        };
        let parameters = callee
            .signature
            .parameters
            .iter()
            .filter_map(|parameter| parameter_name(parameter))
            .collect::<Vec<_>>();

        let receiver_parameter = receiver_values.get(&callee_id).cloned();
        let receiver_index = receiver_parameter
            .as_ref()
            .map(|(_, parameter_name, ordinal)| {
                parameters
                    .iter()
                    .position(|name| name == parameter_name)
                    .unwrap_or(*ordinal)
            });

        if is_receiver_call(call_site, callee) {
            match (
                receiver_parameter.clone(),
                call_receiver_values.get(&calls.call_site_id).cloned(),
            ) {
                (Some((parameter_value_id, parameter_name, ordinal)), Some(argument_value_id)) => {
                    let (confidence, precision) = receiver_parameter_in_confidence(&calls);
                    add_parameter_in(
                        graph,
                        &edge,
                        &calls,
                        &callee_id,
                        &argument_value_id,
                        &parameter_value_id,
                        &parameter_name,
                        ordinal,
                        confidence,
                        precision,
                    );
                    if let Some(state_value_id) = mutable_state_values.get(&argument_value_id) {
                        add_parameter_out_summaries(
                            graph,
                            &edge,
                            &calls,
                            &callee_id,
                            &parameter_value_id,
                            state_value_id,
                            &parameter_name,
                            ordinal,
                            &parameter_out_summaries,
                        );
                    } else if parameter_out_summaries.contains_key(&parameter_value_id) {
                        add_parameter_out_unavailable_diagnostic(
                            graph,
                            &edge,
                            &calls,
                            call_site,
                            &parameter_name,
                            "caller receiver mutable-state value is unavailable",
                        );
                    }
                }
                (None, Some(_)) => add_receiver_unavailable_diagnostic(
                    graph,
                    &edge,
                    &calls,
                    call_site,
                    "callee receiver/self/this value is unavailable",
                ),
                (Some(_), None) => add_receiver_unavailable_diagnostic(
                    graph,
                    &edge,
                    &calls,
                    call_site,
                    "call-site receiver value is unavailable",
                ),
                (None, None) => add_receiver_unavailable_diagnostic(
                    graph,
                    &edge,
                    &calls,
                    call_site,
                    "receiver values are unavailable at the call site and callee",
                ),
            }
        }

        let (confidence, precision) = parameter_in_confidence(&calls);
        let mut positional_ordinal = 0;
        let total_argument_count = call_site.argument_shape.positional_count
            + call_site.argument_shape.named_arguments.len();
        let has_named_formal_match =
            call_site
                .argument_shape
                .named_arguments
                .iter()
                .any(|argument_name| {
                    parameters
                        .iter()
                        .any(|parameter| parameter == argument_name)
                });
        let positional_fallback_count = if has_named_formal_match {
            call_site.argument_shape.positional_count
        } else {
            total_argument_count
        };
        for (parameter_ordinal, parameter_name) in parameters.iter().enumerate() {
            if receiver_index == Some(parameter_ordinal) {
                continue;
            }
            let shape_unknown = call_site.argument_shape.positional_count == 0
                && call_site.argument_shape.named_arguments.is_empty()
                && !parameters.is_empty();
            let matched_argument = named_argument_values
                .get(&(calls.call_site_id.clone(), parameter_name.clone()))
                .map(|(argument_value_id, argument_ordinal)| {
                    (argument_value_id, *argument_ordinal, true)
                })
                .or_else(|| {
                    if positional_ordinal < positional_fallback_count || shape_unknown {
                        call_argument_values
                            .get(&(calls.call_site_id.clone(), positional_ordinal))
                            .map(|argument_value_id| (argument_value_id, positional_ordinal, false))
                    } else {
                        None
                    }
                });
            let Some((argument_value_id, argument_ordinal, matched_by_name)) = matched_argument
            else {
                continue;
            };
            let Some(parameter_value_id) =
                parameter_values.get(&(callee_id.clone(), parameter_name.clone()))
            else {
                add_formal_unavailable_diagnostic(
                    graph,
                    &edge,
                    &calls,
                    call_site,
                    "callee formal parameter value is unavailable",
                );
                continue;
            };
            let precision = if shape_unknown {
                "sg080-argument-shape-unavailable-actual-to-formal"
            } else {
                precision
            };
            add_parameter_in(
                graph,
                &edge,
                &calls,
                &callee_id,
                argument_value_id,
                parameter_value_id,
                parameter_name,
                argument_ordinal,
                confidence,
                precision,
            );
            if let Some(state_value_id) = mutable_state_values.get(argument_value_id) {
                add_parameter_out_summaries(
                    graph,
                    &edge,
                    &calls,
                    &callee_id,
                    parameter_value_id,
                    state_value_id,
                    parameter_name,
                    argument_ordinal,
                    &parameter_out_summaries,
                );
            } else if parameter_out_summaries.contains_key(parameter_value_id) {
                add_parameter_out_unavailable_diagnostic(
                    graph,
                    &edge,
                    &calls,
                    call_site,
                    parameter_name,
                    "caller argument mutable-state value is unavailable",
                );
            }
            if !matched_by_name && argument_ordinal == positional_ordinal {
                positional_ordinal += 1;
            }
        }

        let callee_return_values = return_values.get(&callee_id).cloned().unwrap_or_default();
        match (
            call_result_values.get(&calls.call_site_id),
            callee_return_values.is_empty(),
        ) {
            (Some(call_result_id), false) => {
                let (confidence, precision) = returns_to_confidence(&calls);
                for return_id in &callee_return_values {
                    add_returns_to(
                        graph,
                        &edge,
                        &calls,
                        &callee_id,
                        return_id,
                        call_result_id,
                        confidence,
                        precision,
                    );
                }
            }
            (Some(_), true) => add_return_unavailable_diagnostic(
                graph,
                &edge,
                &calls,
                call_site,
                "callee return value summaries are unavailable",
            ),
            (None, false) => add_return_unavailable_diagnostic(
                graph,
                &edge,
                &calls,
                call_site,
                "caller call-result value is unavailable",
            ),
            (None, true) => {}
        }
        if let Some(call_exception_id) = call_exception_values.get(&calls.call_site_id) {
            let callee_exception_values = exception_values
                .get(&callee_id)
                .cloned()
                .unwrap_or_default();
            if callee_exception_values.is_empty() {
                add_throws_unavailable_diagnostic(
                    graph,
                    &edge,
                    &calls,
                    call_site,
                    "callee exception value summaries are unavailable",
                );
                continue;
            }

            for exception_id in &callee_exception_values {
                let summary = exception_summary(
                    exception_id,
                    &values_by_id,
                    value_spans_by_id.get(exception_id).copied(),
                );
                let destinations =
                    exception_destinations(graph, &callable_index, &callee_id, exception_id, summary.span);
                for handler_id in &destinations.callee_handlers {
                    add_throws_to(
                        graph,
                        &edge,
                        &calls,
                        &callee_id,
                        exception_id,
                        handler_id,
                        Confidence::Exact,
                        SG084_CALLEE_HANDLER_PRECISION,
                        sg::ThrowsToTargetKind::Handler,
                        &summary,
                    );
                }

                if destinations.reaches_callee_exceptional_exit {
                    if let Some(exit_id) = destinations.callee_exceptional_exit.as_deref() {
                        let (confidence, call_precision) = throws_to_confidence(&calls);
                        add_throws_to(
                            graph,
                            &edge,
                            &calls,
                            &callee_id,
                            exception_id,
                            exit_id,
                            confidence,
                            SG084_CALLEE_EXCEPTIONAL_EXIT_PRECISION,
                            sg::ThrowsToTargetKind::CalleeExceptionalExit,
                            &summary,
                        );
                        add_throws_to(
                            graph,
                            &edge,
                            &calls,
                            &callee_id,
                            exception_id,
                            call_exception_id,
                            confidence,
                            call_precision,
                            sg::ThrowsToTargetKind::CallExceptionalValue,
                            &summary,
                        );
                        match caller_exception_destination(graph, &callable_index, call_site) {
                            Some(CallerExceptionDestination::Handler(handler_id)) => {
                                add_throws_to(
                                    graph,
                                    &edge,
                                    &calls,
                                    &callee_id,
                                    exception_id,
                                    &handler_id,
                                    confidence,
                                    SG084_CALLER_HANDLER_PRECISION,
                                    sg::ThrowsToTargetKind::Handler,
                                    &summary,
                                );
                            }
                            Some(CallerExceptionDestination::ExceptionalExit(exit_id)) => {
                                add_throws_to(
                                    graph,
                                    &edge,
                                    &calls,
                                    &callee_id,
                                    exception_id,
                                    &exit_id,
                                    confidence,
                                    SG084_CALLER_EXCEPTIONAL_EXIT_PRECISION,
                                    sg::ThrowsToTargetKind::CallerExceptionalExit,
                                    &summary,
                                );
                            }
                            None => add_throws_unavailable_diagnostic(
                                graph,
                                &edge,
                                &calls,
                                call_site,
                                "caller exceptional CFG destination is unavailable",
                            ),
                        }
                    }
                } else if destinations.callee_handlers.is_empty() {
                    add_throws_unavailable_diagnostic(
                        graph,
                        &edge,
                        &calls,
                        call_site,
                        "callee exception path has no handler or exceptional exit",
                    );
                }
            }
        }
    }
}

fn add_parameter_in(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    callee_id: &str,
    argument_value_id: &str,
    parameter_node_id: &str,
    parameter_name: &str,
    ordinal: usize,
    confidence: Confidence,
    precision: &str,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id(
                "parameter-in",
                argument_value_id,
                parameter_node_id,
                &format!("{ordinal}:{precision}"),
            ),
            EdgeKind::ParameterIn,
            argument_value_id.to_string(),
            parameter_node_id.to_string(),
            call_edge.owner.clone(),
            call_edge.span,
            confidence,
            inference_evidence("resolved call site mapped to callee formal parameter by signature"),
            EdgeFact::ParameterIn(sg::ParameterIn {
                call_site_id: calls.call_site_id.clone(),
                caller_callable_id: calls.caller_callable_id.clone(),
                callee_callable_id: callee_id.to_string(),
                parameter_name: parameter_name.to_string(),
                ordinal,
                precision: precision.to_string(),
            }),
        ),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ParameterOutMutationKind {
    Field,
    Index,
    Alias,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ParameterOutSummary {
    access_value_id: NodeId,
    kind: ParameterOutMutationKind,
}

fn parameter_out_summaries(
    graph: &ProgramSupergraph,
    values_by_id: &BTreeMap<NodeId, sg::Value>,
    expressions_by_id: &BTreeMap<NodeId, sg::Expression>,
) -> BTreeMap<NodeId, Vec<ParameterOutSummary>> {
    let formal_values = values_by_id
        .values()
        .filter(|value| matches!(value.role, ValueRole::FormalParameter | ValueRole::Receiver))
        .filter_map(|value| {
            Some((
                (value.callable_id.clone()?, value.name.clone()?),
                value.value_id.clone(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut summaries = BTreeMap::<NodeId, Vec<ParameterOutSummary>>::new();
    let mut direct_writes = BTreeMap::<NodeId, (NodeId, ParameterOutMutationKind)>::new();

    for edge in graph.edges.iter().filter_map(data_flow_edge) {
        let Some(kind) = direct_mutation_kind(edge.data_flow) else {
            continue;
        };
        let Some(target_id) = edge.edge.target_id.as_deref() else {
            continue;
        };
        let Some((callable_id, base_name)) =
            access_base_name(target_id, values_by_id, expressions_by_id)
        else {
            continue;
        };
        let Some(formal_value_id) = formal_values.get(&(callable_id, base_name)).cloned() else {
            continue;
        };
        direct_writes.insert(target_id.to_string(), (formal_value_id.clone(), kind));
        summaries
            .entry(formal_value_id)
            .or_default()
            .push(ParameterOutSummary {
                access_value_id: target_id.to_string(),
                kind,
            });
    }

    for edge in graph.edges.iter().filter_map(data_flow_edge) {
        if edge.data_flow.precision != "sg073-possible-alias-summary" {
            continue;
        }
        let Some((source_id, target_id)) = edge
            .edge
            .target_id
            .as_deref()
            .map(|target_id| (edge.edge.source_id.as_str(), target_id))
        else {
            continue;
        };
        let Some((direct_formal_id, _)) = direct_writes.get(source_id) else {
            continue;
        };
        let Some((callable_id, base_name)) =
            access_base_name(target_id, values_by_id, expressions_by_id)
        else {
            continue;
        };
        let Some(alias_formal_id) = formal_values.get(&(callable_id, base_name)).cloned() else {
            continue;
        };
        if &alias_formal_id == direct_formal_id {
            continue;
        }
        summaries
            .entry(alias_formal_id)
            .or_default()
            .push(ParameterOutSummary {
                access_value_id: target_id.to_string(),
                kind: ParameterOutMutationKind::Alias,
            });
    }

    for summaries in summaries.values_mut() {
        let mut seen = BTreeSet::new();
        summaries.retain(|summary| seen.insert(summary.clone()));
    }
    summaries
}

struct DataFlowEdge<'a> {
    edge: &'a sg::GraphEdge,
    data_flow: &'a sg::DataFlow,
}

fn data_flow_edge(edge: &sg::GraphEdge) -> Option<DataFlowEdge<'_>> {
    let EdgeFact::DataFlow(data_flow) = &edge.fact else {
        return None;
    };
    Some(DataFlowEdge { edge, data_flow })
}

fn direct_mutation_kind(data_flow: &sg::DataFlow) -> Option<ParameterOutMutationKind> {
    match (data_flow.flow_kind, data_flow.precision.as_str()) {
        (DataFlowKind::FieldAccess, "sg073-field-write-value") => {
            Some(ParameterOutMutationKind::Field)
        }
        (DataFlowKind::IndexAccess, "sg073-index-write-value") => {
            Some(ParameterOutMutationKind::Index)
        }
        _ => None,
    }
}

fn access_base_name(
    value_id: &str,
    values_by_id: &BTreeMap<NodeId, sg::Value>,
    expressions_by_id: &BTreeMap<NodeId, sg::Expression>,
) -> Option<(NodeId, String)> {
    let value = values_by_id.get(value_id)?;
    let expression = value
        .expression_id
        .as_deref()
        .and_then(|expression_id| expressions_by_id.get(expression_id))?;
    if !matches!(
        expression.kind,
        ExpressionKind::FieldAccess | ExpressionKind::IndexAccess
    ) {
        return None;
    }
    let base_expression = expression
        .child_expression_ids
        .first()
        .and_then(|expression_id| expressions_by_id.get(expression_id))?;
    let base_name = base_expression
        .normalized
        .identifier
        .clone()
        .or_else(|| base_expression.original_text.clone())?;
    let trimmed = base_name.trim();
    (!trimmed.is_empty()).then(|| (expression.callable_id.clone(), trimmed.to_string()))
}

fn actual_argument_count(
    call_argument_values: &BTreeMap<(NodeId, usize), NodeId>,
    call_site_id: &str,
) -> usize {
    call_argument_values
        .range((call_site_id.to_string(), 0)..=(call_site_id.to_string(), usize::MAX))
        .count()
}

fn parameter_in_confidence(calls: &sg::Calls) -> (Confidence, &'static str) {
    match calls.resolution {
        Resolution::Exact => (Confidence::Exact, SG080_EXACT_PRECISION),
        Resolution::Probable | Resolution::Possible | Resolution::Ambiguous => {
            (Confidence::Probable, SG080_POSSIBLE_PRECISION)
        }
        Resolution::External | Resolution::Unresolved | Resolution::Unsupported => {
            (Confidence::Unknown, SG080_UNAVAILABLE_PRECISION)
        }
    }
}

fn receiver_parameter_in_confidence(calls: &sg::Calls) -> (Confidence, &'static str) {
    match calls.resolution {
        Resolution::Exact => (Confidence::Exact, SG081_EXACT_PRECISION),
        Resolution::Probable | Resolution::Possible | Resolution::Ambiguous => {
            (Confidence::Probable, SG081_POSSIBLE_PRECISION)
        }
        Resolution::External | Resolution::Unresolved | Resolution::Unsupported => {
            (Confidence::Unknown, SG080_UNAVAILABLE_PRECISION)
        }
    }
}

fn returns_to_confidence(calls: &sg::Calls) -> (Confidence, &'static str) {
    match calls.resolution {
        Resolution::Exact => (Confidence::Exact, SG082_EXACT_PRECISION),
        Resolution::Probable | Resolution::Possible | Resolution::Ambiguous => {
            (Confidence::Probable, SG082_POSSIBLE_PRECISION)
        }
        Resolution::External | Resolution::Unresolved | Resolution::Unsupported => (
            Confidence::Unknown,
            "sg082-return-summary-unavailable-for-call-target",
        ),
    }
}

fn is_receiver_call(call_site: &sg::CallSite, callee: &sg::Callable) -> bool {
    matches!(
        call_site.dispatch_kind,
        DispatchKind::Method | DispatchKind::Constructor
    ) && matches!(
        callee.kind,
        sg::CallableKind::Method | sg::CallableKind::Constructor
    )
}

fn has_receiver_flow_input(
    call_site: &sg::CallSite,
    call_receiver_values: &BTreeMap<NodeId, NodeId>,
    call_site_id: &str,
) -> bool {
    matches!(
        call_site.dispatch_kind,
        DispatchKind::Method | DispatchKind::Constructor
    ) || call_receiver_values.contains_key(call_site_id)
}

fn unavailable_target_reason(calls: &sg::Calls) -> &'static str {
    match calls.resolution {
        Resolution::External => "external call target has no local formal parameter values",
        Resolution::Unresolved => {
            "unresolved call target has no recoverable formal parameter values"
        }
        Resolution::Possible | Resolution::Probable | Resolution::Ambiguous => {
            "possible dynamic call target is not a callable with formal parameter values"
        }
        Resolution::Unsupported => "unsupported call target has no formal parameter values",
        Resolution::Exact => "resolved call target has no formal parameter values",
    }
}

fn unavailable_return_target_reason(calls: &sg::Calls) -> &'static str {
    match calls.resolution {
        Resolution::External => "external call target has no local return value summaries",
        Resolution::Unresolved => {
            "unresolved call target has no recoverable return value summaries"
        }
        Resolution::Possible | Resolution::Probable | Resolution::Ambiguous => {
            "possible dynamic call target is not a callable with return value summaries"
        }
        Resolution::Unsupported => "unsupported call target has no return value summaries",
        Resolution::Exact => "resolved call target has no return value summaries",
    }
}

fn unavailable_exception_target_reason(calls: &sg::Calls) -> &'static str {
    match calls.resolution {
        Resolution::External => "external call target has no local exception value summaries",
        Resolution::Unresolved => {
            "unresolved call target has no recoverable exception value summaries"
        }
        Resolution::Possible | Resolution::Probable | Resolution::Ambiguous => {
            "possible dynamic call target is not a callable with exception value summaries"
        }
        Resolution::Unsupported => "unsupported call target has no exception value summaries",
        Resolution::Exact => "resolved call target has no exception value summaries",
    }
}

fn add_formal_unavailable_diagnostic(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    call_site: &sg::CallSite,
    reason: &str,
) {
    let diagnostic_id = sg::stable_id(
        "diagnostic",
        &[
            "sg080-formal-unavailable",
            &calls.call_site_id,
            call_edge.target_id.as_deref().unwrap_or("unknown-target"),
            reason,
        ],
    );
    let diagnostic_kind = match calls.resolution {
        Resolution::External => DiagnosticKind::ExternalTargetUnknownPackage,
        Resolution::Unresolved => DiagnosticKind::UnresolvedSymbol,
        Resolution::Unsupported => DiagnosticKind::UnsupportedSyntax,
        _ => DiagnosticKind::DynamicDispatch,
    };
    insert_node(
        graph,
        graph_node(
            diagnostic_id.clone(),
            NodeKind::Diagnostic,
            call_edge.owner.clone(),
            call_edge.span.or(Some(call_site.span)),
            Confidence::Unknown,
            inference_evidence("SG-080 could not map actual arguments to local formals"),
            NodeFact::Diagnostic(sg::Diagnostic {
                diagnostic_id,
                kind: diagnostic_kind,
                severity: Severity::Warning,
                message: format!(
                    "SG-080 did not emit ParameterIn for `{}`: {reason}.",
                    call_site.callee_expression
                ),
                artifact_id: call_edge
                    .owner
                    .artifact_id
                    .clone()
                    .or_else(|| Some(call_site.artifact_id.clone())),
                span: call_edge.span.or(Some(call_site.span)),
                related: vec![calls.call_site_id.clone(), call_edge.edge_id.clone()],
            }),
        ),
    );
}

fn add_receiver_unavailable_diagnostic(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    call_site: &sg::CallSite,
    reason: &str,
) {
    let diagnostic_id = sg::stable_id(
        "diagnostic",
        &[
            "sg081-receiver-unavailable",
            &calls.call_site_id,
            call_edge.target_id.as_deref().unwrap_or("unknown-target"),
            reason,
        ],
    );
    let diagnostic_kind = match calls.resolution {
        Resolution::External => DiagnosticKind::ExternalTargetUnknownPackage,
        Resolution::Unresolved => DiagnosticKind::UnresolvedSymbol,
        Resolution::Unsupported => DiagnosticKind::UnsupportedSyntax,
        _ => DiagnosticKind::DynamicDispatch,
    };
    insert_node(
        graph,
        graph_node(
            diagnostic_id.clone(),
            NodeKind::Diagnostic,
            call_edge.owner.clone(),
            call_edge.span.or(Some(call_site.span)),
            Confidence::Unknown,
            inference_evidence("SG-081 could not map call receiver to callee receiver value"),
            NodeFact::Diagnostic(sg::Diagnostic {
                diagnostic_id,
                kind: diagnostic_kind,
                severity: Severity::Warning,
                message: format!(
                    "SG-081 did not emit receiver ParameterIn for `{}`: {reason}.",
                    call_site.callee_expression
                ),
                artifact_id: call_edge
                    .owner
                    .artifact_id
                    .clone()
                    .or_else(|| Some(call_site.artifact_id.clone())),
                span: call_edge.span.or(Some(call_site.span)),
                related: vec![calls.call_site_id.clone(), call_edge.edge_id.clone()],
            }),
        ),
    );
}

fn add_return_unavailable_diagnostic(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    call_site: &sg::CallSite,
    reason: &str,
) {
    let diagnostic_id = sg::stable_id(
        "diagnostic",
        &[
            "sg082-return-unavailable",
            &calls.call_site_id,
            call_edge.target_id.as_deref().unwrap_or("unknown-target"),
            reason,
        ],
    );
    let diagnostic_kind = match calls.resolution {
        Resolution::External => DiagnosticKind::ExternalTargetUnknownPackage,
        Resolution::Unresolved => DiagnosticKind::UnresolvedSymbol,
        Resolution::Unsupported => DiagnosticKind::UnsupportedSyntax,
        _ => DiagnosticKind::DynamicDispatch,
    };
    insert_node(
        graph,
        graph_node(
            diagnostic_id.clone(),
            NodeKind::Diagnostic,
            call_edge.owner.clone(),
            call_edge.span.or(Some(call_site.span)),
            Confidence::Unknown,
            inference_evidence("SG-082 could not map callee returns to the call result value"),
            NodeFact::Diagnostic(sg::Diagnostic {
                diagnostic_id,
                kind: diagnostic_kind,
                severity: Severity::Warning,
                message: format!(
                    "SG-082 did not emit ReturnsTo for `{}`: {reason}.",
                    call_site.callee_expression
                ),
                artifact_id: call_edge
                    .owner
                    .artifact_id
                    .clone()
                    .or_else(|| Some(call_site.artifact_id.clone())),
                span: call_edge.span.or(Some(call_site.span)),
                related: vec![calls.call_site_id.clone(), call_edge.edge_id.clone()],
            }),
        ),
    );
}

fn add_throws_unavailable_diagnostic(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    call_site: &sg::CallSite,
    reason: &str,
) {
    let diagnostic_id = sg::stable_id(
        "diagnostic",
        &[
            "sg084-throws-unavailable",
            &calls.call_site_id,
            call_edge.target_id.as_deref().unwrap_or("unknown-target"),
            reason,
        ],
    );
    let diagnostic_kind = match calls.resolution {
        Resolution::External => DiagnosticKind::ExternalTargetUnknownPackage,
        Resolution::Unresolved => DiagnosticKind::UnresolvedSymbol,
        Resolution::Unsupported => DiagnosticKind::UnsupportedSyntax,
        _ => DiagnosticKind::DynamicDispatch,
    };
    insert_node(
        graph,
        graph_node(
            diagnostic_id.clone(),
            NodeKind::Diagnostic,
            call_edge.owner.clone(),
            call_edge.span.or(Some(call_site.span)),
            Confidence::Unknown,
            inference_evidence("SG-084 could not map exception flow precisely"),
            NodeFact::Diagnostic(sg::Diagnostic {
                diagnostic_id,
                kind: diagnostic_kind,
                severity: Severity::Warning,
                message: format!(
                    "SG-084 did not emit precise ThrowsTo for `{}`: {reason}.",
                    call_site.callee_expression
                ),
                artifact_id: call_edge
                    .owner
                    .artifact_id
                    .clone()
                    .or_else(|| Some(call_site.artifact_id.clone())),
                span: call_edge.span.or(Some(call_site.span)),
                related: vec![calls.call_site_id.clone(), call_edge.edge_id.clone()],
            }),
        ),
    );
}

fn add_parameter_out_summaries(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    callee_id: &str,
    parameter_node_id: &str,
    state_value_id: &str,
    parameter_name: &str,
    ordinal: usize,
    parameter_out_summaries: &BTreeMap<NodeId, Vec<ParameterOutSummary>>,
) {
    let Some(summaries) = parameter_out_summaries.get(parameter_node_id) else {
        return;
    };
    for summary in summaries {
        let (confidence, precision) = parameter_out_confidence(calls, summary.kind);
        insert_edge(
            graph,
            graph_edge(
                edge_id(
                    "parameter-out",
                    parameter_node_id,
                    state_value_id,
                    &format!("{}:{}:{}", ordinal, summary.access_value_id, precision),
                ),
                EdgeKind::ParameterOut,
                parameter_node_id.to_string(),
                state_value_id.to_string(),
                call_edge.owner.clone(),
                call_edge.span,
                confidence,
                inference_evidence(
                    "callee field/index mutation summarized to caller-visible argument state",
                ),
                EdgeFact::ParameterOut(sg::ParameterOut {
                    call_site_id: calls.call_site_id.clone(),
                    caller_callable_id: calls.caller_callable_id.clone(),
                    callee_callable_id: callee_id.to_string(),
                    parameter_name: parameter_name.to_string(),
                    ordinal,
                    precision: precision.to_string(),
                }),
            ),
        );
    }
}

fn parameter_out_confidence(
    calls: &sg::Calls,
    kind: ParameterOutMutationKind,
) -> (Confidence, &'static str) {
    match calls.resolution {
        Resolution::Exact => match kind {
            ParameterOutMutationKind::Field => (Confidence::Exact, SG083_FIELD_PRECISION),
            ParameterOutMutationKind::Index => (Confidence::Exact, SG083_INDEX_PRECISION),
            ParameterOutMutationKind::Alias => (Confidence::Unknown, SG083_ALIAS_PRECISION),
        },
        Resolution::Probable | Resolution::Possible | Resolution::Ambiguous => match kind {
            ParameterOutMutationKind::Field => {
                (Confidence::Probable, SG083_POSSIBLE_CALL_FIELD_PRECISION)
            }
            ParameterOutMutationKind::Index => {
                (Confidence::Probable, SG083_POSSIBLE_CALL_INDEX_PRECISION)
            }
            ParameterOutMutationKind::Alias => {
                (Confidence::Unknown, SG083_POSSIBLE_CALL_ALIAS_PRECISION)
            }
        },
        Resolution::External | Resolution::Unresolved | Resolution::Unsupported => (
            Confidence::Unknown,
            "sg083-parameter-out-summary-unavailable-for-call-target",
        ),
    }
}

fn add_parameter_out_unavailable_diagnostic(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    call_site: &sg::CallSite,
    parameter_name: &str,
    reason: &str,
) {
    let diagnostic_id = sg::stable_id(
        "diagnostic",
        &[
            "sg083-parameter-out-unavailable",
            &calls.call_site_id,
            parameter_name,
            reason,
        ],
    );
    insert_node(
        graph,
        graph_node(
            diagnostic_id.clone(),
            NodeKind::Diagnostic,
            call_edge.owner.clone(),
            call_edge.span.or(Some(call_site.span)),
            Confidence::Unknown,
            inference_evidence(
                "SG-083 could not connect a callee mutation summary to caller state",
            ),
            NodeFact::Diagnostic(sg::Diagnostic {
                diagnostic_id,
                kind: DiagnosticKind::UnsupportedSyntax,
                severity: Severity::Warning,
                message: format!(
                    "SG-083 did not emit ParameterOut for `{}` parameter `{}`: {reason}.",
                    call_site.callee_expression, parameter_name
                ),
                artifact_id: call_edge
                    .owner
                    .artifact_id
                    .clone()
                    .or_else(|| Some(call_site.artifact_id.clone())),
                span: call_edge.span.or(Some(call_site.span)),
                related: vec![calls.call_site_id.clone(), call_edge.edge_id.clone()],
            }),
        ),
    );
}

fn add_returns_to(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    callee_id: &str,
    return_id: &str,
    call_result_id: &str,
    confidence: Confidence,
    precision: &str,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id(
                "returns-to",
                return_id,
                call_result_id,
                &format!("{callee_id}:{precision}"),
            ),
            EdgeKind::ReturnsTo,
            return_id.to_string(),
            call_result_id.to_string(),
            call_edge.owner.clone(),
            call_edge.span,
            confidence,
            inference_evidence("resolved local call receives explicit callee return"),
            EdgeFact::ReturnsTo(sg::ReturnsTo {
                call_site_id: calls.call_site_id.clone(),
                caller_callable_id: calls.caller_callable_id.clone(),
                callee_callable_id: callee_id.to_string(),
                precision: precision.to_string(),
            }),
        ),
    );
}

fn add_throws_to(
    graph: &mut ProgramSupergraph,
    call_edge: &sg::GraphEdge,
    calls: &sg::Calls,
    callee_id: &str,
    raise_id: &str,
    target_id: &str,
    confidence: Confidence,
    precision: &str,
    target_kind: sg::ThrowsToTargetKind,
    summary: &ExceptionSummary,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id(
                "throws-to",
                raise_id,
                target_id,
                &format!("{callee_id}:{precision}"),
            ),
            EdgeKind::ThrowsTo,
            raise_id.to_string(),
            target_id.to_string(),
            call_edge.owner.clone(),
            call_edge.span,
            confidence,
            inference_evidence("SG-084 mapped exceptional value flow across call boundary"),
            EdgeFact::ThrowsTo(sg::ThrowsTo {
                call_site_id: calls.call_site_id.clone(),
                caller_callable_id: calls.caller_callable_id.clone(),
                callee_callable_id: callee_id.to_string(),
                target_kind,
                exception_value: summary.value.clone(),
                exception_type: summary.exception_type.clone(),
                precision: precision.to_string(),
            }),
        ),
    );
}

#[derive(Debug, Clone)]
struct ExceptionSummary {
    value: Option<String>,
    exception_type: Option<String>,
    span: Option<SourceSpan>,
}

#[derive(Debug, Default)]
struct ExceptionDestinations {
    callee_handlers: Vec<NodeId>,
    callee_exceptional_exit: Option<NodeId>,
    reaches_callee_exceptional_exit: bool,
}

enum CallerExceptionDestination {
    Handler(NodeId),
    ExceptionalExit(NodeId),
}

fn exception_summary(
    exception_id: &str,
    values_by_id: &BTreeMap<NodeId, sg::Value>,
    span: Option<SourceSpan>,
) -> ExceptionSummary {
    let value = values_by_id
        .get(exception_id)
        .and_then(|value| value.name.clone());
    let exception_type = value.as_deref().and_then(exception_type_from_value);
    ExceptionSummary {
        value,
        exception_type,
        span,
    }
}

fn exception_type_from_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let trimmed = trimmed
        .strip_prefix("raise ")
        .or_else(|| trimmed.strip_prefix("throw "))
        .unwrap_or(trimmed)
        .trim();
    let trimmed = trimmed.strip_prefix("new ").unwrap_or(trimmed).trim();
    let candidate = trimmed
        .split(['(', '{', '[', ' ', ';'])
        .next()
        .unwrap_or(trimmed)
        .trim();
    if candidate.is_empty()
        || candidate.starts_with('"')
        || candidate.starts_with('\'')
        || candidate
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
    {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn throws_to_confidence(calls: &sg::Calls) -> (Confidence, &'static str) {
    match calls.resolution {
        Resolution::Exact => (Confidence::Exact, SG084_EXACT_CALL_EXCEPTION_PRECISION),
        Resolution::Probable | Resolution::Possible | Resolution::Ambiguous => (
            Confidence::Probable,
            SG084_POSSIBLE_CALL_EXCEPTION_PRECISION,
        ),
        Resolution::External | Resolution::Unresolved | Resolution::Unsupported => (
            Confidence::Unknown,
            "sg084-exception-summary-unavailable-for-call-target",
        ),
    }
}

fn exception_destinations(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callee_id: &str,
    exception_id: &str,
    exception_span: Option<SourceSpan>,
) -> ExceptionDestinations {
    let Some(raise_node_id) = exception_span.and_then(|span| {
        cfg_node_at_span(
            graph,
            index,
            callee_id,
            span,
            Some(ControlFlowNodeRole::Raise),
        )
    }) else {
        return ExceptionDestinations::default();
    };
    let callee_exceptional_exit = exceptional_exit_for_callable(graph, index, callee_id);
    let reaches_callee_exceptional_exit = callee_exceptional_exit
        .as_deref()
        .is_some_and(|exit_id| cfg_reachable(graph, index, callee_id, &raise_node_id, exit_id));
    let mut callee_handlers = index
        .control_flow_edges(graph, callee_id)
        .filter_map(|edge| {
            let EdgeFact::ControlFlow(flow) = &edge.fact else {
                return None;
            };
            if edge.source_id == raise_node_id && flow.outcome == sg::ControlFlowOutcome::Exception
            {
                edge.target_id.clone()
            } else {
                None
            }
        })
        .filter(|target_id| {
            callee_exceptional_exit.as_deref() != Some(target_id.as_str())
                && !reaches_callee_exceptional_exit
        })
        .collect::<Vec<_>>();
    callee_handlers.sort();
    callee_handlers.dedup();
    if callee_handlers.is_empty() && !reaches_callee_exceptional_exit {
        // The span matched an exceptional value, so keep the target visible in diagnostics.
        callee_handlers.push(format!("{exception_id}:handler-unavailable"));
        callee_handlers.clear();
    }
    ExceptionDestinations {
        callee_handlers,
        callee_exceptional_exit,
        reaches_callee_exceptional_exit,
    }
}

fn caller_exception_destination(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    call_site: &sg::CallSite,
) -> Option<CallerExceptionDestination> {
    caller_handler_for_call_site(graph, index, call_site)
        .map(CallerExceptionDestination::Handler)
        .or_else(|| {
            exceptional_exit_for_callable(graph, index, &call_site.enclosing_callable_id)
                .map(CallerExceptionDestination::ExceptionalExit)
        })
}

fn caller_handler_for_call_site(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    call_site: &sg::CallSite,
) -> Option<NodeId> {
    let statement_id = smallest_statement_containing_span(
        graph,
        index,
        &call_site.enclosing_callable_id,
        call_site.span,
    )?;
    index
        .nodes(graph, call_site.enclosing_callable_id)
        .find_map(|node| {
            let NodeFact::Condition(condition) = &node.fact else {
                return None;
            };
            if condition.kind != sg::ConditionKind::ExceptionRegion {
                return None;
            }
            let protects_call = condition.regions.iter().any(|region| {
                region.kind == ControlRegionKind::TryBody
                    && region.statement_ids.contains(&statement_id)
            });
            if !protects_call {
                return None;
            }
            let handler_statement_id = condition
                .regions
                .iter()
                .find(|region| region.kind == ControlRegionKind::CatchBody)?
                .statement_ids
                .first()?;
            cfg_node_for_statement_id(
                graph,
                index,
                &call_site.enclosing_callable_id,
                handler_statement_id,
            )
        })
}

fn smallest_statement_containing_span(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    span: SourceSpan,
) -> Option<NodeId> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| {
            let NodeFact::Statement(statement) = &node.fact else {
                return None;
            };
            let node_span = node.span?;
            if span_contains(node_span, span) {
                Some((
                    node_span.end_byte.saturating_sub(node_span.start_byte),
                    statement.statement_id.clone(),
                ))
            } else {
                None
            }
        })
        .min_by_key(|(width, statement_id)| (*width, statement_id.clone()))
        .map(|(_, statement_id)| statement_id)
}

fn cfg_node_for_statement_id(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    statement_id: &str,
) -> Option<NodeId> {
    let statement_span = index.nodes(graph, callable_id).find_map(|node| {
        let NodeFact::Statement(statement) = &node.fact else {
            return None;
        };
        (statement.statement_id == statement_id).then_some(node.span?)
    })?;
    cfg_node_at_span(
        graph,
        index,
        callable_id,
        statement_span,
        Some(ControlFlowNodeRole::Statement),
    )
}

fn cfg_node_at_span(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    span: SourceSpan,
    role: Option<ControlFlowNodeRole>,
) -> Option<NodeId> {
    index.nodes(graph, callable_id).find_map(|node| {
        let NodeFact::ControlFlow(control) = &node.fact else {
            return None;
        };
        if node.span == Some(span) && role.is_none_or(|role| control.role == role) {
            return Some(control.cfg_node_id.clone());
        }
        None
    })
}

fn exceptional_exit_for_callable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
) -> Option<NodeId> {
    index.nodes(graph, callable_id).find_map(|node| {
        let NodeFact::ControlFlow(control) = &node.fact else {
            return None;
        };
        if control.role == ControlFlowNodeRole::Exit
            && control.semantic_kind.as_deref() == Some("ExceptionalExit")
        {
            Some(control.cfg_node_id.clone())
        } else {
            None
        }
    })
}

fn cfg_reachable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    source_id: &str,
    target_id: &str,
) -> bool {
    let mut successors = HashMap::<&str, Vec<&str>>::new();
    for edge in index.control_flow_edges(graph, callable_id) {
        if let Some(next) = &edge.target_id {
            successors
                .entry(edge.source_id.as_str())
                .or_default()
                .push(next.as_str());
        }
    }
    let mut seen = HashSet::new();
    let mut frontier = vec![source_id];
    while let Some(node_id) = frontier.pop() {
        if node_id == target_id {
            return true;
        }
        if !seen.insert(node_id) {
            continue;
        }
        if let Some(next) = successors.get(node_id) {
            frontier.extend(next.iter().copied());
        }
    }
    false
}

fn span_contains(container: SourceSpan, child: SourceSpan) -> bool {
    container.start_byte <= child.start_byte && child.end_byte <= container.end_byte
}

fn parameter_name(parameter: &str) -> Option<String> {
    let trimmed = parameter.trim();
    let without_default = trimmed.split('=').next().unwrap_or(trimmed).trim();
    let without_type = without_default
        .split(':')
        .next()
        .unwrap_or(without_default)
        .trim();
    let without_optional = without_type.trim_end_matches('?').trim();
    without_optional
        .split_whitespace()
        .last()
        .map(str::to_string)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{CallContext, SourceSpan};
    use crate::supergraph::{
        ArgumentShape, CallEdgeKind, Callable, CallableKind, ControlFlowNodeRole, DataFlowKind,
        ExternalTarget, ExternalTargetKind, GraphEdge, GraphNode, ProgramDependenceGraphView,
        ProgramSupergraphBuilder, Signature, SourceOwnership, Uncertainty, Value, ValueKind,
        build_indexes, refresh_fact_identity, refresh_provenance, refresh_uncertainty, sort_graph,
        stable_id,
    };

    #[test]
    fn sg080_python_connects_positional_and_named_actuals_to_formals() {
        let mut graph = sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut graph,
            "call:python:positional",
            "process",
            ArgumentShape {
                positional_count: 2,
                named_arguments: Vec::new(),
            },
            vec![
                argument_value(
                    "value:python:positional:first",
                    "call:python:positional",
                    0,
                    None,
                ),
                argument_value(
                    "value:python:positional:second",
                    "call:python:positional",
                    1,
                    None,
                ),
            ],
            Resolution::Exact,
        );
        add_local_call(
            &mut graph,
            "call:python:named",
            "process",
            ArgumentShape {
                positional_count: 0,
                named_arguments: vec!["second".to_string()],
            },
            vec![argument_value(
                "value:python:named:second",
                "call:python:named",
                0,
                Some("second"),
            )],
            Resolution::Exact,
        );
        finish_sg080_emit(&mut graph);

        assert_parameter_in(
            &graph,
            "call:python:positional",
            "value:python:positional:first",
            "value:callee:first",
            "first",
            Confidence::Exact,
            Uncertainty::Exact,
        );
        assert_parameter_in(
            &graph,
            "call:python:positional",
            "value:python:positional:second",
            "value:callee:second",
            "second",
            Confidence::Exact,
            Uncertainty::Exact,
        );
        assert_parameter_in(
            &graph,
            "call:python:named",
            "value:python:named:second",
            "value:callee:second",
            "second",
            Confidence::Exact,
            Uncertainty::Exact,
        );
        assert!(
            !parameter_in_edges_for_call(&graph, "call:python:named")
                .iter()
                .any(|edge| edge.target_id.as_deref() == Some("value:callee:first")),
            "named argument should not be consumed as a positional argument for the first formal"
        );
        assert_parameter_in_endpoints_are_values(&graph);
    }

    #[test]
    fn sg080_typescript_handles_possible_and_unavailable_call_targets() {
        let mut graph = sg080_graph("typescript", "sample.ts", "tree-sitter-typescript@0.23");
        add_local_call(
            &mut graph,
            "call:typescript:possible",
            "handler",
            ArgumentShape {
                positional_count: 1,
                named_arguments: Vec::new(),
            },
            vec![argument_value(
                "value:typescript:possible:first",
                "call:typescript:possible",
                0,
                None,
            )],
            Resolution::Possible,
        );
        add_external_call(
            &mut graph,
            "call:typescript:external",
            "externalHandler",
            "external:typescript:handler",
            Resolution::External,
        );
        add_external_call(
            &mut graph,
            "call:typescript:unresolved",
            "missingHandler",
            "external:typescript:missing",
            Resolution::Unresolved,
        );
        add_possible_dynamic_binding_call(&mut graph);
        finish_sg080_emit(&mut graph);

        assert_parameter_in(
            &graph,
            "call:typescript:possible",
            "value:typescript:possible:first",
            "value:callee:first",
            "first",
            Confidence::Probable,
            Uncertainty::Possible,
        );
        assert!(parameter_in_edges_for_call(&graph, "call:typescript:external").is_empty());
        assert!(parameter_in_edges_for_call(&graph, "call:typescript:unresolved").is_empty());
        assert!(parameter_in_edges_for_call(&graph, "call:typescript:dynamic").is_empty());
        assert_diagnostic_for_call(
            &graph,
            "call:typescript:external",
            DiagnosticKind::ExternalTargetUnknownPackage,
        );
        assert_diagnostic_for_call(
            &graph,
            "call:typescript:unresolved",
            DiagnosticKind::UnresolvedSymbol,
        );
        assert_diagnostic_for_call(
            &graph,
            "call:typescript:dynamic",
            DiagnosticKind::DynamicDispatch,
        );
        assert_parameter_in_endpoints_are_values(&graph);
    }

    #[test]
    fn sg081_python_method_receiver_flows_to_self_value() {
        let mut graph = sg081_graph(
            "python",
            "sample.py",
            "tree-sitter-python@0.23",
            CallableKind::Method,
            vec!["self".to_string(), "item".to_string()],
            "self",
        );
        add_receiver_local_call(
            &mut graph,
            "call:python:method",
            "store.add",
            DispatchKind::Method,
            CallEdgeKind::Method,
            Resolution::Exact,
            "value:python:method:receiver",
            Some(argument_value(
                "value:python:method:item",
                "call:python:method",
                0,
                None,
            )),
        );
        finish_sg080_emit(&mut graph);

        assert_receiver_parameter_in(
            &graph,
            "call:python:method",
            "value:python:method:receiver",
            "value:callee:receiver:self",
            "self",
            Confidence::Exact,
            Uncertainty::Exact,
            SG081_EXACT_PRECISION,
        );
        assert_parameter_in(
            &graph,
            "call:python:method",
            "value:python:method:item",
            "value:callee:formal:item",
            "item",
            Confidence::Exact,
            Uncertainty::Exact,
        );
        assert!(
            !parameter_in_edges_for_call(&graph, "call:python:method")
                .iter()
                .any(|edge| {
                    edge.source_id == "value:python:method:item"
                        && edge.target_id.as_deref() == Some("value:callee:receiver:self")
                }),
            "ordinary arguments must not be mapped to the receiver formal"
        );
        assert_parameter_in_endpoints_are_values(&graph);
    }

    #[test]
    fn sg081_typescript_constructor_receiver_flows_to_this_when_present() {
        let mut graph = sg081_graph(
            "typescript",
            "sample.ts",
            "tree-sitter-typescript@0.23",
            CallableKind::Constructor,
            vec!["this".to_string(), "name".to_string()],
            "this",
        );
        add_receiver_local_call(
            &mut graph,
            "call:typescript:constructor",
            "new Widget",
            DispatchKind::Constructor,
            CallEdgeKind::Constructor,
            Resolution::Exact,
            "value:typescript:constructor:new-object",
            Some(argument_value(
                "value:typescript:constructor:name",
                "call:typescript:constructor",
                0,
                None,
            )),
        );
        finish_sg080_emit(&mut graph);

        assert_receiver_parameter_in(
            &graph,
            "call:typescript:constructor",
            "value:typescript:constructor:new-object",
            "value:callee:receiver:this",
            "this",
            Confidence::Exact,
            Uncertainty::Exact,
            SG081_EXACT_PRECISION,
        );
        assert_parameter_in(
            &graph,
            "call:typescript:constructor",
            "value:typescript:constructor:name",
            "value:callee:formal:name",
            "name",
            Confidence::Exact,
            Uncertainty::Exact,
        );
        assert_parameter_in_endpoints_are_values(&graph);
    }

    #[test]
    fn sg081_possible_method_target_marks_receiver_flow_possible() {
        let mut graph = sg081_graph(
            "typescript",
            "sample.ts",
            "tree-sitter-typescript@0.23",
            CallableKind::Method,
            vec!["this".to_string()],
            "this",
        );
        add_receiver_local_call(
            &mut graph,
            "call:typescript:possible-method",
            "maybe.handle",
            DispatchKind::Method,
            CallEdgeKind::Method,
            Resolution::Possible,
            "value:typescript:possible-method:receiver",
            None,
        );
        finish_sg080_emit(&mut graph);

        assert_receiver_parameter_in(
            &graph,
            "call:typescript:possible-method",
            "value:typescript:possible-method:receiver",
            "value:callee:receiver:this",
            "this",
            Confidence::Probable,
            Uncertainty::Possible,
            SG081_POSSIBLE_PRECISION,
        );
        assert_parameter_in_endpoints_are_values(&graph);
    }

    #[test]
    fn sg081_unavailable_receiver_targets_emit_diagnostics_without_edges() {
        let mut graph = sg081_graph(
            "typescript",
            "sample.ts",
            "tree-sitter-typescript@0.23",
            CallableKind::Method,
            vec!["this".to_string()],
            "this",
        );
        add_external_receiver_call(
            &mut graph,
            "call:typescript:external-method",
            "remote.handle",
            "external:typescript:remote-handle",
            Resolution::External,
        );
        add_external_receiver_call(
            &mut graph,
            "call:typescript:unresolved-method",
            "missing.handle",
            "external:typescript:missing-handle",
            Resolution::Unresolved,
        );
        add_receiver_dynamic_binding_call(&mut graph);
        finish_sg080_emit(&mut graph);

        assert!(parameter_in_edges_for_call(&graph, "call:typescript:external-method").is_empty());
        assert!(
            parameter_in_edges_for_call(&graph, "call:typescript:unresolved-method").is_empty()
        );
        assert!(parameter_in_edges_for_call(&graph, "call:typescript:dynamic-method").is_empty());
        assert_diagnostic_for_call(
            &graph,
            "call:typescript:external-method",
            DiagnosticKind::ExternalTargetUnknownPackage,
        );
        assert_diagnostic_for_call(
            &graph,
            "call:typescript:unresolved-method",
            DiagnosticKind::UnresolvedSymbol,
        );
        assert_diagnostic_for_call(
            &graph,
            "call:typescript:dynamic-method",
            DiagnosticKind::DynamicDispatch,
        );
    }

    #[test]
    fn sg081_missing_receiver_endpoint_emits_diagnostic_without_invented_edge() {
        let mut graph = sg081_graph(
            "python",
            "sample.py",
            "tree-sitter-python@0.23",
            CallableKind::Method,
            vec!["self".to_string()],
            "self",
        );
        add_receiver_local_call_without_receiver_value(
            &mut graph,
            "call:python:missing-receiver",
            "store.add",
            DispatchKind::Method,
            CallEdgeKind::Method,
            Resolution::Exact,
        );
        add_receiver_local_call(
            &mut graph,
            "call:python:missing-formal",
            "store.missing",
            DispatchKind::Method,
            CallEdgeKind::Method,
            Resolution::Exact,
            "value:python:missing-formal:receiver",
            None,
        );
        graph.nodes.retain(|node| {
            !matches!(
                &node.fact,
                NodeFact::Value(value)
                    if value.value_id == "value:callee:receiver:self"
            )
        });
        finish_sg080_emit(&mut graph);

        assert!(parameter_in_edges_for_call(&graph, "call:python:missing-receiver").is_empty());
        assert!(parameter_in_edges_for_call(&graph, "call:python:missing-formal").is_empty());
        assert_diagnostic_for_call(
            &graph,
            "call:python:missing-receiver",
            DiagnosticKind::DynamicDispatch,
        );
        assert_diagnostic_for_call(
            &graph,
            "call:python:missing-formal",
            DiagnosticKind::DynamicDispatch,
        );
    }

    #[test]
    fn sg082_python_connects_callee_return_to_call_result_and_slices_through_use() {
        let mut graph = sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut graph,
            "call:python:return",
            "process",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Exact,
        );
        let caller_owner = graph_owner(&graph);
        graph.nodes.push(value_node(
            return_value("value:callee:return:python", "callable:callee", "out"),
            owner("artifact:sample.py", "scope:sample", "callable:callee"),
        ));
        graph.nodes.push(value_node(
            call_result_value("value:python:call-result", "call:python:return", "process"),
            caller_owner.clone(),
        ));
        graph.nodes.push(value_node(
            caller_use_value("value:python:caller-use", "received"),
            caller_owner.clone(),
        ));
        add_caller_use_flow(
            &mut graph,
            "value:python:call-result",
            "value:python:caller-use",
        );
        finish_sg080_emit(&mut graph);

        assert_returns_to(
            &graph,
            "call:python:return",
            "value:callee:return:python",
            "value:python:call-result",
            Confidence::Exact,
            Uncertainty::Exact,
            SG082_EXACT_PRECISION,
        );
        let view = ProgramDependenceGraphView::new(&graph);
        let backward = view.behavior_backward_slice("value:python:call-result");
        assert!(
            backward
                .value_ids
                .contains(&"value:callee:return:python".to_string())
        );
        let forward = view.value_forward_slice("value:callee:return:python");
        assert!(
            forward
                .value_ids
                .contains(&"value:python:call-result".to_string())
        );
        assert!(
            forward
                .value_ids
                .contains(&"value:python:caller-use".to_string())
        );
        assert_returns_to_endpoints_are_values(&graph);
    }

    #[test]
    fn sg082_typescript_marks_possible_local_return_summary_uncertain() {
        let mut graph = sg080_graph("typescript", "sample.ts", "tree-sitter-typescript@0.23");
        add_local_call(
            &mut graph,
            "call:typescript:possible-return",
            "handler",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Possible,
        );
        let caller_owner = graph_owner(&graph);
        graph.nodes.push(value_node(
            return_value(
                "value:callee:return:typescript",
                "callable:callee",
                "result",
            ),
            owner("artifact:sample.ts", "scope:sample", "callable:callee"),
        ));
        graph.nodes.push(value_node(
            call_result_value(
                "value:typescript:possible-call-result",
                "call:typescript:possible-return",
                "handler",
            ),
            caller_owner,
        ));
        finish_sg080_emit(&mut graph);

        assert_returns_to(
            &graph,
            "call:typescript:possible-return",
            "value:callee:return:typescript",
            "value:typescript:possible-call-result",
            Confidence::Probable,
            Uncertainty::Possible,
            SG082_POSSIBLE_PRECISION,
        );
    }

    #[test]
    fn sg082_external_unresolved_and_dynamic_targets_emit_diagnostics_without_returns() {
        let mut graph = sg080_graph("typescript", "sample.ts", "tree-sitter-typescript@0.23");
        add_external_call(
            &mut graph,
            "call:typescript:external-return",
            "externalHandler",
            "external:typescript:return-handler",
            Resolution::External,
        );
        add_external_call(
            &mut graph,
            "call:typescript:unresolved-return",
            "missingHandler",
            "external:typescript:missing-return-handler",
            Resolution::Unresolved,
        );
        add_possible_dynamic_binding_call(&mut graph);
        let caller_owner = graph_owner(&graph);
        for (call_site_id, value_id, name) in [
            (
                "call:typescript:external-return",
                "value:typescript:external-call-result",
                "externalHandler",
            ),
            (
                "call:typescript:unresolved-return",
                "value:typescript:unresolved-call-result",
                "missingHandler",
            ),
            (
                "call:typescript:dynamic",
                "value:typescript:dynamic-call-result",
                "dynamicHandler",
            ),
        ] {
            graph.nodes.push(value_node(
                call_result_value(value_id, call_site_id, name),
                caller_owner.clone(),
            ));
        }
        finish_sg080_emit(&mut graph);

        assert!(returns_to_edges_for_call(&graph, "call:typescript:external-return").is_empty());
        assert!(returns_to_edges_for_call(&graph, "call:typescript:unresolved-return").is_empty());
        assert!(returns_to_edges_for_call(&graph, "call:typescript:dynamic").is_empty());
        assert_sg082_diagnostic_for_call(
            &graph,
            "call:typescript:external-return",
            DiagnosticKind::ExternalTargetUnknownPackage,
        );
        assert_sg082_diagnostic_for_call(
            &graph,
            "call:typescript:unresolved-return",
            DiagnosticKind::UnresolvedSymbol,
        );
        assert_sg082_diagnostic_for_call(
            &graph,
            "call:typescript:dynamic",
            DiagnosticKind::DynamicDispatch,
        );
    }

    #[test]
    fn sg082_missing_return_or_call_result_endpoint_emits_diagnostic() {
        let mut missing_return_graph =
            sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut missing_return_graph,
            "call:python:missing-return",
            "process",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Exact,
        );
        let caller_owner = graph_owner(&missing_return_graph);
        missing_return_graph.nodes.push(value_node(
            call_result_value(
                "value:python:missing-return-call-result",
                "call:python:missing-return",
                "process",
            ),
            caller_owner,
        ));
        finish_sg080_emit(&mut missing_return_graph);

        assert!(
            returns_to_edges_for_call(&missing_return_graph, "call:python:missing-return")
                .is_empty()
        );
        assert_sg082_diagnostic_for_call(
            &missing_return_graph,
            "call:python:missing-return",
            DiagnosticKind::DynamicDispatch,
        );

        let mut missing_result_graph =
            sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut missing_result_graph,
            "call:python:missing-call-result",
            "process",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Exact,
        );
        missing_result_graph.nodes.push(value_node(
            return_value(
                "value:callee:return:missing-result",
                "callable:callee",
                "out",
            ),
            owner("artifact:sample.py", "scope:sample", "callable:callee"),
        ));
        finish_sg080_emit(&mut missing_result_graph);

        assert!(
            returns_to_edges_for_call(&missing_result_graph, "call:python:missing-call-result")
                .is_empty()
        );
        assert_sg082_diagnostic_for_call(
            &missing_result_graph,
            "call:python:missing-call-result",
            DiagnosticKind::DynamicDispatch,
        );
    }

    #[test]
    fn sg084_python_routes_unhandled_callee_exception_to_caller_except_handler() {
        let mut graph = sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut graph,
            "call:python:raise-to-handler",
            "process",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Exact,
        );
        let caller_owner = graph_owner(&graph);
        graph.nodes.push(value_node(
            exception_value(
                "value:callee:exception:value-error",
                "callable:callee",
                "ValueError()",
            ),
            owner("artifact:sample.py", "scope:sample", "callable:callee"),
        ));
        graph.nodes.push(value_node(
            call_exception_value(
                "value:python:call-exception",
                "call:python:raise-to-handler",
                "process",
            ),
            caller_owner.clone(),
        ));
        add_callee_unhandled_exception_cfg(
            &mut graph,
            "value:callee:exception:value-error",
            "ValueError()",
        );
        add_caller_try_catch_cfg(
            &mut graph,
            "call:python:raise-to-handler",
            "cfg:caller:python:except-handler",
            "Except",
        );
        finish_sg080_emit(&mut graph);

        assert_throws_to(
            &graph,
            "call:python:raise-to-handler",
            "value:callee:exception:value-error",
            "value:python:call-exception",
            Confidence::Exact,
            Uncertainty::Exact,
            sg::ThrowsToTargetKind::CallExceptionalValue,
            SG084_EXACT_CALL_EXCEPTION_PRECISION,
            Some("ValueError"),
        );
        assert_throws_to(
            &graph,
            "call:python:raise-to-handler",
            "value:callee:exception:value-error",
            "cfg:caller:python:except-handler",
            Confidence::Exact,
            Uncertainty::Exact,
            sg::ThrowsToTargetKind::Handler,
            SG084_CALLER_HANDLER_PRECISION,
            Some("ValueError"),
        );
        assert!(
            !throws_to_edges_for_call(&graph, "call:python:raise-to-handler")
                .iter()
                .any(|edge| {
                    matches!(
                        &edge.fact,
                        EdgeFact::ThrowsTo(throws)
                            if throws.target_kind == sg::ThrowsToTargetKind::CallerExceptionalExit
                    )
                }),
            "handled caller exceptions must not flow directly to caller exceptional exit"
        );
        let view = ProgramDependenceGraphView::new(&graph);
        let forward = view.value_forward_slice("value:callee:exception:value-error");
        assert!(
            forward
                .value_ids
                .contains(&"value:python:call-exception".to_string())
        );
    }

    #[test]
    fn sg084_typescript_marks_possible_unhandled_exception_to_exit_uncertain() {
        let mut graph = sg080_graph("typescript", "sample.ts", "tree-sitter-typescript@0.23");
        add_local_call(
            &mut graph,
            "call:typescript:possible-throw",
            "handler",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Possible,
        );
        graph.nodes.push(value_node(
            exception_value(
                "value:callee:exception:error",
                "callable:callee",
                "new Error()",
            ),
            owner("artifact:sample.ts", "scope:sample", "callable:callee"),
        ));
        graph.nodes.push(value_node(
            call_exception_value(
                "value:typescript:possible-call-exception",
                "call:typescript:possible-throw",
                "handler",
            ),
            graph_owner(&graph),
        ));
        add_callee_unhandled_exception_cfg(
            &mut graph,
            "value:callee:exception:error",
            "new Error()",
        );
        add_caller_exceptional_exit_cfg(
            &mut graph,
            "callable:caller",
            "cfg:caller:ts:exceptional-exit",
        );
        finish_sg080_emit(&mut graph);

        assert_throws_to(
            &graph,
            "call:typescript:possible-throw",
            "value:callee:exception:error",
            "value:typescript:possible-call-exception",
            Confidence::Probable,
            Uncertainty::Possible,
            sg::ThrowsToTargetKind::CallExceptionalValue,
            SG084_POSSIBLE_CALL_EXCEPTION_PRECISION,
            Some("Error"),
        );
        assert_throws_to(
            &graph,
            "call:typescript:possible-throw",
            "value:callee:exception:error",
            "cfg:caller:ts:exceptional-exit",
            Confidence::Probable,
            Uncertainty::Possible,
            sg::ThrowsToTargetKind::CallerExceptionalExit,
            SG084_CALLER_EXCEPTIONAL_EXIT_PRECISION,
            Some("Error"),
        );
    }

    #[test]
    fn sg084_callee_handled_exception_does_not_escape_to_caller_exit() {
        let mut graph = sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut graph,
            "call:python:handled-in-callee",
            "process",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            Vec::new(),
            Resolution::Exact,
        );
        graph.nodes.push(value_node(
            exception_value(
                "value:callee:exception:handled",
                "callable:callee",
                "ValueError()",
            ),
            owner("artifact:sample.py", "scope:sample", "callable:callee"),
        ));
        graph.nodes.push(value_node(
            call_exception_value(
                "value:python:handled-call-exception",
                "call:python:handled-in-callee",
                "process",
            ),
            graph_owner(&graph),
        ));
        add_callee_handled_exception_cfg(
            &mut graph,
            "value:callee:exception:handled",
            "ValueError()",
            "cfg:callee:except-handler",
        );
        add_caller_exceptional_exit_cfg(
            &mut graph,
            "callable:caller",
            "cfg:caller:py:exceptional-exit",
        );
        finish_sg080_emit(&mut graph);

        assert_throws_to(
            &graph,
            "call:python:handled-in-callee",
            "value:callee:exception:handled",
            "cfg:callee:except-handler",
            Confidence::Exact,
            Uncertainty::Exact,
            sg::ThrowsToTargetKind::Handler,
            SG084_CALLEE_HANDLER_PRECISION,
            Some("ValueError"),
        );
        assert!(
            !throws_to_edges_for_call(&graph, "call:python:handled-in-callee")
                .iter()
                .any(|edge| edge.target_id.as_deref()
                    == Some("value:python:handled-call-exception")
                    || matches!(
                        &edge.fact,
                        EdgeFact::ThrowsTo(throws)
                            if throws.target_kind == sg::ThrowsToTargetKind::CallerExceptionalExit
                    )),
            "callee-handled exceptions must not be projected to caller exceptional paths"
        );
    }

    #[test]
    fn sg084_external_and_unresolved_exception_targets_emit_diagnostics() {
        let mut graph = sg080_graph("typescript", "sample.ts", "tree-sitter-typescript@0.23");
        add_external_call(
            &mut graph,
            "call:typescript:external-throw",
            "externalHandler",
            "external:typescript:throw-handler",
            Resolution::External,
        );
        add_external_call(
            &mut graph,
            "call:typescript:unresolved-throw",
            "missingHandler",
            "external:typescript:missing-throw-handler",
            Resolution::Unresolved,
        );
        let owner = graph_owner(&graph);
        graph.nodes.push(value_node(
            call_exception_value(
                "value:typescript:external-call-exception",
                "call:typescript:external-throw",
                "externalHandler",
            ),
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            call_exception_value(
                "value:typescript:unresolved-call-exception",
                "call:typescript:unresolved-throw",
                "missingHandler",
            ),
            owner,
        ));
        finish_sg080_emit(&mut graph);

        assert!(throws_to_edges_for_call(&graph, "call:typescript:external-throw").is_empty());
        assert!(throws_to_edges_for_call(&graph, "call:typescript:unresolved-throw").is_empty());
        assert_sg084_diagnostic_for_call(
            &graph,
            "call:typescript:external-throw",
            DiagnosticKind::ExternalTargetUnknownPackage,
        );
        assert_sg084_diagnostic_for_call(
            &graph,
            "call:typescript:unresolved-throw",
            DiagnosticKind::UnresolvedSymbol,
        );
    }

    #[test]
    fn sg083_python_does_not_emit_parameter_out_without_callee_mutation() {
        let mut graph = sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut graph,
            "call:python:no-mutation",
            "process",
            ArgumentShape {
                positional_count: 1,
                named_arguments: Vec::new(),
            },
            vec![argument_value(
                "value:python:no-mutation:first",
                "call:python:no-mutation",
                0,
                None,
            )],
            Resolution::Exact,
        );
        add_mutable_state_value(
            &mut graph,
            "value:python:no-mutation:first-state",
            "value:python:no-mutation:first",
            "call:python:no-mutation",
            "first",
            0,
        );
        finish_sg080_emit(&mut graph);

        assert!(parameter_out_edges_for_call(&graph, "call:python:no-mutation").is_empty());
    }

    #[test]
    fn sg083_python_emits_receiver_field_mutation_summary() {
        let mut graph = sg081_graph(
            "python",
            "sample.py",
            "tree-sitter-python@0.23",
            CallableKind::Method,
            vec!["self".to_string(), "item".to_string()],
            "self",
        );
        add_receiver_local_call(
            &mut graph,
            "call:python:receiver-mutation",
            "store.add",
            DispatchKind::Method,
            CallEdgeKind::Method,
            Resolution::Exact,
            "value:python:receiver-mutation:receiver",
            Some(argument_value(
                "value:python:receiver-mutation:item",
                "call:python:receiver-mutation",
                0,
                None,
            )),
        );
        add_mutable_state_value(
            &mut graph,
            "value:python:receiver-mutation:receiver-state",
            "value:python:receiver-mutation:receiver",
            "call:python:receiver-mutation",
            "store",
            0,
        );
        add_mutable_state_value(
            &mut graph,
            "value:python:receiver-mutation:item-state",
            "value:python:receiver-mutation:item",
            "call:python:receiver-mutation",
            "item",
            0,
        );
        add_callee_access_write(
            &mut graph,
            "receiver-field",
            "self",
            "items",
            ExpressionKind::FieldAccess,
        );
        finish_sg080_emit(&mut graph);

        assert_parameter_out(
            &graph,
            "call:python:receiver-mutation",
            "value:callee:receiver:self",
            "value:python:receiver-mutation:receiver-state",
            "self",
            Confidence::Exact,
            Uncertainty::Exact,
            SG083_FIELD_PRECISION,
        );
        assert!(
            !parameter_out_edges_for_call(&graph, "call:python:receiver-mutation")
                .iter()
                .any(|edge| edge.target_id.as_deref()
                    == Some("value:python:receiver-mutation:item-state")),
            "pass-through method arguments must not receive ParameterOut"
        );
    }

    #[test]
    fn sg083_typescript_emits_argument_field_and_index_mutation_summaries() {
        let mut graph = sg080_graph("typescript", "sample.ts", "tree-sitter-typescript@0.23");
        add_local_call(
            &mut graph,
            "call:typescript:object-mutation",
            "process",
            ArgumentShape {
                positional_count: 2,
                named_arguments: Vec::new(),
            },
            vec![
                argument_value(
                    "value:typescript:object-mutation:first",
                    "call:typescript:object-mutation",
                    0,
                    None,
                ),
                argument_value(
                    "value:typescript:object-mutation:second",
                    "call:typescript:object-mutation",
                    1,
                    None,
                ),
            ],
            Resolution::Exact,
        );
        add_mutable_state_value(
            &mut graph,
            "value:typescript:object-mutation:first-state",
            "value:typescript:object-mutation:first",
            "call:typescript:object-mutation",
            "first",
            0,
        );
        add_mutable_state_value(
            &mut graph,
            "value:typescript:object-mutation:second-state",
            "value:typescript:object-mutation:second",
            "call:typescript:object-mutation",
            "second",
            1,
        );
        add_callee_access_write(
            &mut graph,
            "first-field",
            "first",
            "value",
            ExpressionKind::FieldAccess,
        );
        add_callee_access_write(
            &mut graph,
            "second-index",
            "second",
            "0",
            ExpressionKind::IndexAccess,
        );
        finish_sg080_emit(&mut graph);

        assert_parameter_out(
            &graph,
            "call:typescript:object-mutation",
            "value:callee:first",
            "value:typescript:object-mutation:first-state",
            "first",
            Confidence::Exact,
            Uncertainty::Exact,
            SG083_FIELD_PRECISION,
        );
        assert_parameter_out(
            &graph,
            "call:typescript:object-mutation",
            "value:callee:second",
            "value:typescript:object-mutation:second-state",
            "second",
            Confidence::Exact,
            Uncertainty::Exact,
            SG083_INDEX_PRECISION,
        );
    }

    #[test]
    fn sg083_python_marks_possible_alias_mutation_without_broad_outputs() {
        let mut graph = sg080_graph("python", "sample.py", "tree-sitter-python@0.23");
        add_local_call(
            &mut graph,
            "call:python:alias-mutation",
            "process",
            ArgumentShape {
                positional_count: 2,
                named_arguments: Vec::new(),
            },
            vec![
                argument_value(
                    "value:python:alias-mutation:first",
                    "call:python:alias-mutation",
                    0,
                    None,
                ),
                argument_value(
                    "value:python:alias-mutation:second",
                    "call:python:alias-mutation",
                    1,
                    None,
                ),
            ],
            Resolution::Exact,
        );
        add_mutable_state_value(
            &mut graph,
            "value:python:alias-mutation:first-state",
            "value:python:alias-mutation:first",
            "call:python:alias-mutation",
            "first",
            0,
        );
        add_mutable_state_value(
            &mut graph,
            "value:python:alias-mutation:second-state",
            "value:python:alias-mutation:second",
            "call:python:alias-mutation",
            "second",
            1,
        );
        let write_value = add_callee_access_write(
            &mut graph,
            "alias-write",
            "first",
            "value",
            ExpressionKind::FieldAccess,
        );
        let alias_read_value = add_callee_access_value(
            &mut graph,
            "alias-read",
            "second",
            "value",
            ExpressionKind::FieldAccess,
        );
        add_alias_summary_flow(&mut graph, &write_value, &alias_read_value);
        finish_sg080_emit(&mut graph);

        assert_parameter_out(
            &graph,
            "call:python:alias-mutation",
            "value:callee:first",
            "value:python:alias-mutation:first-state",
            "first",
            Confidence::Exact,
            Uncertainty::Exact,
            SG083_FIELD_PRECISION,
        );
        assert_parameter_out(
            &graph,
            "call:python:alias-mutation",
            "value:callee:second",
            "value:python:alias-mutation:second-state",
            "second",
            Confidence::Unknown,
            Uncertainty::Possible,
            SG083_ALIAS_PRECISION,
        );
    }

    fn sg080_graph(language: &str, path: &str, _parser_version: &str) -> ProgramSupergraph {
        let artifact_id = format!("artifact:{path}");
        let scope_id = "scope:sample".to_string();
        let caller_id = "callable:caller".to_string();
        let callee_id = "callable:callee".to_string();
        let owner = owner(&artifact_id, &scope_id, &caller_id);
        let mut builder = ProgramSupergraphBuilder::new("repo", language);
        builder.add_artifact(
            sg::Artifact {
                artifact_id: artifact_id.clone(),
                path: path.to_string(),
                module_path: "sample".to_string(),
                content_hash: Some(format!("sha256:sg080:{language}")),
            },
            Vec::new(),
        );
        builder.add_scope(
            sg::Scope {
                scope_id: scope_id.clone(),
                parent_scope_id: None,
                artifact_id: artifact_id.clone(),
                kind: sg::ScopeKind::Module,
                variant: sg::ScopeVariant::FileModule,
                language_variant: Some(format!("{language}:module")),
                binding_behavior: sg::ScopeBindingBehavior::Boundary,
                owner_callable_id: None,
                span: None,
            },
            Confidence::Exact,
            Vec::new(),
        );
        for (callable_id, name, parameters) in [
            (caller_id.clone(), "caller", Vec::new()),
            (
                callee_id.clone(),
                "process",
                vec!["first".to_string(), "second".to_string()],
            ),
        ] {
            builder.add_callable(
                Callable {
                    callable_id: callable_id.clone(),
                    kind: CallableKind::Function,
                    name: Some(name.to_string()),
                    qualified_name: format!("sample.{name}"),
                    artifact_id: artifact_id.clone(),
                    declaration_span: span(10, 20),
                    body_span: Some(span(21, 80)),
                    signature: Signature {
                        parameters,
                        return_annotation: None,
                    },
                    scope_id: scope_id.clone(),
                    attributes: Vec::new(),
                    incoming_local_call_count: 0,
                    external_invocation_metadata: Vec::new(),
                },
                Confidence::Exact,
                Vec::new(),
            );
        }
        for (value_id, name, ordinal) in [
            ("value:callee:first", "first", 0),
            ("value:callee:second", "second", 1),
        ] {
            builder.add_value(
                formal_value(value_id, &callee_id, name, ordinal),
                owner.clone(),
                Some(span(11 + ordinal, 12 + ordinal)),
                Confidence::Exact,
                inference_evidence("SG-080 test formal parameter value"),
            );
        }
        builder.finish()
    }

    fn add_local_call(
        graph: &mut ProgramSupergraph,
        call_site_id: &str,
        callee_expression: &str,
        argument_shape: ArgumentShape,
        arguments: Vec<Value>,
        resolution: Resolution,
    ) {
        let owner = graph_owner(graph);
        graph.nodes.push(call_site_node(
            call_site_id,
            callee_expression,
            argument_shape,
            owner.clone(),
        ));
        for argument in arguments {
            graph.nodes.push(value_node(argument, owner.clone()));
        }
        graph.edges.push(call_edge(
            call_site_id,
            "callable:callee",
            Some("callable:callee".to_string()),
            None,
            None,
            resolution,
            owner,
        ));
    }

    fn add_external_call(
        graph: &mut ProgramSupergraph,
        call_site_id: &str,
        callee_expression: &str,
        external_target_id: &str,
        resolution: Resolution,
    ) {
        let owner = graph_owner(graph);
        graph.nodes.push(call_site_node(
            call_site_id,
            callee_expression,
            ArgumentShape {
                positional_count: 1,
                named_arguments: Vec::new(),
            },
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            argument_value(
                &format!("value:{call_site_id}:argument"),
                call_site_id,
                0,
                None,
            ),
            owner.clone(),
        ));
        graph.nodes.push(GraphNode {
            node_id: external_target_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::ExternalTarget,
            owner: SourceOwnership::default(),
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: Vec::new(),
            fact: NodeFact::ExternalTarget(ExternalTarget {
                external_target_id: external_target_id.to_string(),
                ecosystem: "typescript".to_string(),
                package_name: Some("external".to_string()),
                package_version: None,
                module_path: Some("external".to_string()),
                qualified_name: callee_expression.to_string(),
                member_path: None,
                target_kind: ExternalTargetKind::Function,
                source: "sg080-test".to_string(),
            }),
        });
        graph.edges.push(call_edge(
            call_site_id,
            external_target_id,
            None,
            (resolution == Resolution::External).then(|| external_target_id.to_string()),
            (resolution == Resolution::Unresolved).then(|| callee_expression.to_string()),
            resolution,
            owner,
        ));
    }

    fn add_possible_dynamic_binding_call(graph: &mut ProgramSupergraph) {
        let owner = graph_owner(graph);
        let call_site_id = "call:typescript:dynamic";
        graph.nodes.push(call_site_node(
            call_site_id,
            "dynamicHandler",
            ArgumentShape {
                positional_count: 1,
                named_arguments: Vec::new(),
            },
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            argument_value("value:typescript:dynamic:first", call_site_id, 0, None),
            owner.clone(),
        ));
        graph.edges.push(call_edge(
            call_site_id,
            "binding:dynamicHandler",
            None,
            None,
            None,
            Resolution::Possible,
            owner,
        ));
    }

    fn sg081_graph(
        language: &str,
        path: &str,
        parser_version: &str,
        callee_kind: CallableKind,
        parameters: Vec<String>,
        receiver_name: &str,
    ) -> ProgramSupergraph {
        let mut graph = sg080_graph(language, path, parser_version);
        for node in &mut graph.nodes {
            if let NodeFact::Callable(callable) = &mut node.fact {
                if callable.callable_id == "callable:callee" {
                    callable.kind = callee_kind;
                    callable.signature.parameters = parameters.clone();
                }
            }
        }
        let owner = graph_owner(&graph);
        graph.nodes.push(value_node(
            receiver_formal_value(
                &format!("value:callee:receiver:{receiver_name}"),
                "callable:callee",
                receiver_name,
                0,
            ),
            owner.clone(),
        ));
        for (ordinal, parameter_name) in parameters.iter().enumerate() {
            if parameter_name != receiver_name {
                graph.nodes.push(value_node(
                    formal_value(
                        &format!("value:callee:formal:{parameter_name}"),
                        "callable:callee",
                        parameter_name,
                        ordinal,
                    ),
                    owner.clone(),
                ));
            }
        }
        graph
    }

    fn add_receiver_local_call(
        graph: &mut ProgramSupergraph,
        call_site_id: &str,
        callee_expression: &str,
        dispatch_kind: DispatchKind,
        call_kind: CallEdgeKind,
        resolution: Resolution,
        receiver_value_id: &str,
        argument: Option<Value>,
    ) {
        let owner = graph_owner(graph);
        let positional_count = usize::from(argument.is_some());
        graph.nodes.push(call_site_node_with_dispatch(
            call_site_id,
            callee_expression,
            ArgumentShape {
                positional_count,
                named_arguments: Vec::new(),
            },
            dispatch_kind,
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            call_receiver_value(receiver_value_id, call_site_id, callee_expression),
            owner.clone(),
        ));
        if let Some(argument) = argument {
            graph.nodes.push(value_node(argument, owner.clone()));
        }
        graph.edges.push(call_edge_with_kind(
            call_site_id,
            "callable:callee",
            Some("callable:callee".to_string()),
            None,
            None,
            resolution,
            call_kind,
            owner,
        ));
    }

    fn add_receiver_local_call_without_receiver_value(
        graph: &mut ProgramSupergraph,
        call_site_id: &str,
        callee_expression: &str,
        dispatch_kind: DispatchKind,
        call_kind: CallEdgeKind,
        resolution: Resolution,
    ) {
        let owner = graph_owner(graph);
        graph.nodes.push(call_site_node_with_dispatch(
            call_site_id,
            callee_expression,
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            dispatch_kind,
            owner.clone(),
        ));
        graph.edges.push(call_edge_with_kind(
            call_site_id,
            "callable:callee",
            Some("callable:callee".to_string()),
            None,
            None,
            resolution,
            call_kind,
            owner,
        ));
    }

    fn add_external_receiver_call(
        graph: &mut ProgramSupergraph,
        call_site_id: &str,
        callee_expression: &str,
        external_target_id: &str,
        resolution: Resolution,
    ) {
        let owner = graph_owner(graph);
        graph.nodes.push(call_site_node_with_dispatch(
            call_site_id,
            callee_expression,
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            DispatchKind::Method,
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            call_receiver_value(
                &format!("value:{call_site_id}:receiver"),
                call_site_id,
                callee_expression,
            ),
            owner.clone(),
        ));
        graph.nodes.push(GraphNode {
            node_id: external_target_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::ExternalTarget,
            owner: SourceOwnership::default(),
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: Vec::new(),
            fact: NodeFact::ExternalTarget(ExternalTarget {
                external_target_id: external_target_id.to_string(),
                ecosystem: "typescript".to_string(),
                package_name: Some("external".to_string()),
                package_version: None,
                module_path: Some("external".to_string()),
                qualified_name: callee_expression.to_string(),
                member_path: None,
                target_kind: ExternalTargetKind::Function,
                source: "sg081-test".to_string(),
            }),
        });
        graph.edges.push(call_edge_with_kind(
            call_site_id,
            external_target_id,
            None,
            (resolution == Resolution::External).then(|| external_target_id.to_string()),
            (resolution == Resolution::Unresolved).then(|| callee_expression.to_string()),
            resolution,
            CallEdgeKind::External,
            owner,
        ));
    }

    fn add_receiver_dynamic_binding_call(graph: &mut ProgramSupergraph) {
        let owner = graph_owner(graph);
        let call_site_id = "call:typescript:dynamic-method";
        graph.nodes.push(call_site_node_with_dispatch(
            call_site_id,
            "dynamic.handle",
            ArgumentShape {
                positional_count: 0,
                named_arguments: Vec::new(),
            },
            DispatchKind::Method,
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            call_receiver_value(
                "value:typescript:dynamic-method:receiver",
                call_site_id,
                "dynamic",
            ),
            owner.clone(),
        ));
        graph.edges.push(call_edge_with_kind(
            call_site_id,
            "binding:dynamicMethod",
            None,
            None,
            None,
            Resolution::Possible,
            CallEdgeKind::PossibleDynamic,
            owner,
        ));
    }

    fn finish_sg080_emit(graph: &mut ProgramSupergraph) {
        emit(graph);
        refresh_uncertainty(graph);
        refresh_provenance(graph);
        refresh_fact_identity(graph);
        sort_graph(graph);
        graph.indexes = build_indexes(&graph.nodes, &graph.edges);
    }

    fn formal_value(value_id: &str, callable_id: &str, name: &str, ordinal: usize) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some(callable_id.to_string()),
            kind: ValueKind::Parameter,
            role: ValueRole::FormalParameter,
            symbol_id: None,
            expression_id: None,
            call_site_id: None,
            name: Some(name.to_string()),
            ordinal: Some(ordinal),
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn receiver_formal_value(
        value_id: &str,
        callable_id: &str,
        name: &str,
        ordinal: usize,
    ) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some(callable_id.to_string()),
            kind: ValueKind::Parameter,
            role: ValueRole::Receiver,
            symbol_id: None,
            expression_id: None,
            call_site_id: None,
            name: Some(name.to_string()),
            ordinal: Some(ordinal),
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn argument_value(
        value_id: &str,
        call_site_id: &str,
        ordinal: usize,
        name: Option<&str>,
    ) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some("callable:caller".to_string()),
            kind: ValueKind::Argument,
            role: ValueRole::Argument,
            symbol_id: None,
            expression_id: None,
            call_site_id: Some(call_site_id.to_string()),
            name: name.map(str::to_string),
            ordinal: Some(ordinal),
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn call_receiver_value(value_id: &str, call_site_id: &str, name: &str) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some("callable:caller".to_string()),
            kind: ValueKind::Argument,
            role: ValueRole::Receiver,
            symbol_id: None,
            expression_id: None,
            call_site_id: Some(call_site_id.to_string()),
            name: Some(name.to_string()),
            ordinal: Some(0),
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn return_value(value_id: &str, callable_id: &str, name: &str) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some(callable_id.to_string()),
            kind: ValueKind::Return,
            role: ValueRole::ReturnValue,
            symbol_id: None,
            expression_id: None,
            call_site_id: None,
            name: Some(name.to_string()),
            ordinal: None,
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn call_result_value(value_id: &str, call_site_id: &str, name: &str) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some("callable:caller".to_string()),
            kind: ValueKind::CallResult,
            role: ValueRole::CallResult,
            symbol_id: None,
            expression_id: None,
            call_site_id: Some(call_site_id.to_string()),
            name: Some(name.to_string()),
            ordinal: None,
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn exception_value(value_id: &str, callable_id: &str, name: &str) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some(callable_id.to_string()),
            kind: ValueKind::Exception,
            role: ValueRole::ExceptionalValue,
            symbol_id: None,
            expression_id: None,
            call_site_id: None,
            name: Some(name.to_string()),
            ordinal: None,
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn call_exception_value(value_id: &str, call_site_id: &str, name: &str) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some("callable:caller".to_string()),
            kind: ValueKind::Exception,
            role: ValueRole::ExceptionalValue,
            symbol_id: None,
            expression_id: None,
            call_site_id: Some(call_site_id.to_string()),
            name: Some(name.to_string()),
            ordinal: None,
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn caller_use_value(value_id: &str, name: &str) -> Value {
        Value {
            value_id: value_id.to_string(),
            callable_id: Some("callable:caller".to_string()),
            kind: ValueKind::ComputedExpression,
            role: ValueRole::Unknown,
            symbol_id: None,
            expression_id: None,
            call_site_id: None,
            name: Some(name.to_string()),
            ordinal: None,
            state_of_value_id: None,
            type_hint: None,
            literal: None,
        }
    }

    fn add_callee_unhandled_exception_cfg(
        graph: &mut ProgramSupergraph,
        exception_value_id: &str,
        label: &str,
    ) {
        let raise_id = "cfg:callee:raise";
        let exit_id = "cfg:callee:exceptional-exit";
        add_cfg_node(
            graph,
            raise_id,
            "callable:callee",
            ControlFlowNodeRole::Raise,
            label,
            None,
            Some(span(101, 102)),
        );
        add_caller_exceptional_exit_cfg(graph, "callable:callee", exit_id);
        add_cfg_edge(
            graph,
            "callable:callee",
            raise_id,
            exit_id,
            sg::ControlFlowOutcome::Exception,
            "sg084-test-unhandled-raise-to-exceptional-exit",
        );
        set_value_span(graph, exception_value_id, span(101, 102));
    }

    fn add_callee_handled_exception_cfg(
        graph: &mut ProgramSupergraph,
        exception_value_id: &str,
        label: &str,
        handler_id: &str,
    ) {
        let raise_id = "cfg:callee:handled-raise";
        add_cfg_node(
            graph,
            raise_id,
            "callable:callee",
            ControlFlowNodeRole::Raise,
            label,
            None,
            Some(span(101, 102)),
        );
        add_cfg_node(
            graph,
            handler_id,
            "callable:callee",
            ControlFlowNodeRole::Statement,
            "recover()",
            Some("Expression"),
            Some(span(150, 160)),
        );
        add_cfg_edge(
            graph,
            "callable:callee",
            raise_id,
            handler_id,
            sg::ControlFlowOutcome::Exception,
            "sg084-test-handled-raise-to-handler",
        );
        set_value_span(graph, exception_value_id, span(101, 102));
    }

    fn add_caller_exceptional_exit_cfg(
        graph: &mut ProgramSupergraph,
        callable_id: &str,
        exit_id: &str,
    ) {
        add_cfg_node(
            graph,
            exit_id,
            callable_id,
            ControlFlowNodeRole::Exit,
            "exceptional exit",
            Some("ExceptionalExit"),
            None,
        );
    }

    fn add_caller_try_catch_cfg(
        graph: &mut ProgramSupergraph,
        call_site_id: &str,
        handler_id: &str,
        handler_kind: &str,
    ) {
        let call_statement_id = "statement:caller:try-call";
        let handler_statement_id = "statement:caller:except-handler";
        let owner = graph_owner(graph);
        graph.nodes.push(GraphNode {
            node_id: call_statement_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::Statement,
            owner: owner.clone(),
            span: Some(span(90, 130)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-084 test caller try statement"),
            fact: NodeFact::Statement(sg::Statement {
                statement_id: call_statement_id.to_string(),
                callable_id: "callable:caller".to_string(),
                parent_statement_id: None,
                kind: sg::StatementKind::Expression,
                ordinal: 0,
                child_statement_ids: Vec::new(),
                expression_ids: Vec::new(),
                control_effects: Vec::new(),
            }),
        });
        graph.nodes.push(GraphNode {
            node_id: handler_statement_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::Statement,
            owner: owner.clone(),
            span: Some(span(130, 145)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-084 test caller handler statement"),
            fact: NodeFact::Statement(sg::Statement {
                statement_id: handler_statement_id.to_string(),
                callable_id: "callable:caller".to_string(),
                parent_statement_id: None,
                kind: sg::StatementKind::Expression,
                ordinal: 1,
                child_statement_ids: Vec::new(),
                expression_ids: Vec::new(),
                control_effects: Vec::new(),
            }),
        });
        graph.nodes.push(GraphNode {
            node_id: "condition:caller:try".to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::Condition,
            owner: owner.clone(),
            span: Some(span(80, 160)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-084 test caller exception region"),
            fact: NodeFact::Condition(sg::Condition {
                condition_id: "condition:caller:try".to_string(),
                callable_id: "callable:caller".to_string(),
                statement_id: None,
                expression_id: None,
                kind: sg::ConditionKind::ExceptionRegion,
                controlled_statement_ids: vec![
                    call_statement_id.to_string(),
                    handler_statement_id.to_string(),
                ],
                outcome_labels: vec!["try".to_string(), "exception".to_string()],
                regions: vec![
                    sg::ControlRegion {
                        kind: ControlRegionKind::TryBody,
                        label: "try-body".to_string(),
                        statement_ids: vec![call_statement_id.to_string()],
                        entry_statement_id: Some(call_statement_id.to_string()),
                        exit_statement_id: Some(call_statement_id.to_string()),
                        fallthrough: sg::FallthroughBehavior::Conditional,
                    },
                    sg::ControlRegion {
                        kind: ControlRegionKind::CatchBody,
                        label: "catch-body".to_string(),
                        statement_ids: vec![handler_statement_id.to_string()],
                        entry_statement_id: Some(handler_statement_id.to_string()),
                        exit_statement_id: Some(handler_statement_id.to_string()),
                        fallthrough: sg::FallthroughBehavior::Conditional,
                    },
                ],
                continuation: None,
                fallthrough: sg::FallthroughBehavior::Conditional,
            }),
        });
        add_cfg_node(
            graph,
            handler_id,
            "callable:caller",
            ControlFlowNodeRole::Statement,
            "recover()",
            Some(handler_kind),
            Some(span(130, 145)),
        );
        add_caller_exceptional_exit_cfg(graph, "callable:caller", "cfg:caller:exceptional-exit");
        assert!(
            graph.nodes.iter().any(|node| {
                matches!(
                    &node.fact,
                    NodeFact::CallSite(call_site) if call_site.call_site_id == call_site_id
                )
            }),
            "test fixture must add call site before caller try/catch CFG"
        );
    }

    fn add_cfg_node(
        graph: &mut ProgramSupergraph,
        node_id: &str,
        callable_id: &str,
        role: ControlFlowNodeRole,
        label: &str,
        semantic_kind: Option<&str>,
        span: Option<SourceSpan>,
    ) {
        graph.nodes.push(GraphNode {
            node_id: node_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::ControlFlow,
            owner: owner(&graph_artifact_id(graph), "scope:sample", callable_id),
            span,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-084 test CFG node"),
            fact: NodeFact::ControlFlow(sg::ControlFlowNode {
                cfg_node_id: node_id.to_string(),
                callable_id: callable_id.to_string(),
                role,
                label: label.to_string(),
                semantic_kind: semantic_kind.map(str::to_string),
            }),
        });
    }

    fn add_cfg_edge(
        graph: &mut ProgramSupergraph,
        callable_id: &str,
        source_id: &str,
        target_id: &str,
        outcome: sg::ControlFlowOutcome,
        precision: &str,
    ) {
        graph.edges.push(GraphEdge {
            edge_id: stable_id("edge", &["sg084-test-cfg", source_id, target_id, precision]),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: EdgeKind::ControlFlow,
            source_id: source_id.to_string(),
            target_id: Some(target_id.to_string()),
            owner: owner(&graph_artifact_id(graph), "scope:sample", callable_id),
            span: Some(span(101, 102)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-084 test exceptional CFG edge"),
            fact: EdgeFact::ControlFlow(sg::ControlFlow {
                callable_id: callable_id.to_string(),
                flow_kind: sg::ControlFlowKind::Branch,
                outcome,
                branch_arm: None,
                precision: precision.to_string(),
            }),
        });
    }

    fn set_value_span(graph: &mut ProgramSupergraph, value_id: &str, span: SourceSpan) {
        for node in &mut graph.nodes {
            if matches!(&node.fact, NodeFact::Value(value) if value.value_id == value_id) {
                node.span = Some(span);
            }
        }
    }

    fn add_caller_use_flow(graph: &mut ProgramSupergraph, source_id: &str, target_id: &str) {
        graph.edges.push(GraphEdge {
            edge_id: stable_id("edge", &["sg082-test-data-flow", source_id, target_id]),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: EdgeKind::DataFlow,
            source_id: source_id.to_string(),
            target_id: Some(target_id.to_string()),
            owner: graph_owner(graph),
            span: Some(span(130, 140)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-082 test caller use data flow"),
            fact: EdgeFact::DataFlow(sg::DataFlow {
                callable_id: "callable:caller".to_string(),
                name: "received".to_string(),
                flow_kind: DataFlowKind::DefinitionToUse,
                precision: "sg082-test-call-result-to-caller-use".to_string(),
            }),
        });
    }

    fn add_mutable_state_value(
        graph: &mut ProgramSupergraph,
        state_value_id: &str,
        state_of_value_id: &str,
        call_site_id: &str,
        name: &str,
        ordinal: usize,
    ) {
        graph.nodes.push(value_node(
            Value {
                value_id: state_value_id.to_string(),
                callable_id: Some("callable:caller".to_string()),
                kind: ValueKind::Argument,
                role: ValueRole::MutableArgumentState,
                symbol_id: None,
                expression_id: None,
                call_site_id: Some(call_site_id.to_string()),
                name: Some(name.to_string()),
                ordinal: Some(ordinal),
                state_of_value_id: Some(state_of_value_id.to_string()),
                type_hint: None,
                literal: None,
            },
            graph_owner(graph),
        ));
    }

    fn add_callee_access_write(
        graph: &mut ProgramSupergraph,
        access_key: &str,
        base_name: &str,
        member_name: &str,
        kind: ExpressionKind,
    ) -> String {
        let access_value_id =
            add_callee_access_value(graph, access_key, base_name, member_name, kind);
        let rhs_value_id = format!("value:callee:{access_key}:rhs");
        let artifact_id = graph_artifact_id(graph);
        graph.nodes.push(value_node(
            Value {
                value_id: rhs_value_id.clone(),
                callable_id: Some("callable:callee".to_string()),
                kind: ValueKind::ComputedExpression,
                role: ValueRole::Unknown,
                symbol_id: None,
                expression_id: None,
                call_site_id: None,
                name: Some(format!("rhs:{access_key}")),
                ordinal: None,
                state_of_value_id: None,
                type_hint: None,
                literal: None,
            },
            owner(&artifact_id, "scope:sample", "callable:callee"),
        ));
        let (flow_kind, precision) = match kind {
            ExpressionKind::FieldAccess => (DataFlowKind::FieldAccess, "sg073-field-write-value"),
            ExpressionKind::IndexAccess => (DataFlowKind::IndexAccess, "sg073-index-write-value"),
            _ => panic!("SG-083 access write helper requires field or index access"),
        };
        graph.edges.push(GraphEdge {
            edge_id: stable_id(
                "edge",
                &["sg083-test-write", &rhs_value_id, &access_value_id],
            ),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: EdgeKind::DataFlow,
            source_id: rhs_value_id,
            target_id: Some(access_value_id.clone()),
            owner: owner(&artifact_id, "scope:sample", "callable:callee"),
            span: Some(span(150, 160)),
            confidence: Confidence::Probable,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-083 test callee access write"),
            fact: EdgeFact::DataFlow(sg::DataFlow {
                callable_id: "callable:callee".to_string(),
                name: member_name.to_string(),
                flow_kind,
                precision: precision.to_string(),
            }),
        });
        access_value_id
    }

    fn add_callee_access_value(
        graph: &mut ProgramSupergraph,
        access_key: &str,
        base_name: &str,
        member_name: &str,
        kind: ExpressionKind,
    ) -> String {
        let artifact_id = graph_artifact_id(graph);
        let owner = owner(&artifact_id, "scope:sample", "callable:callee");
        let base_expression_id = format!("expr:callee:{access_key}:base");
        let member_expression_id = format!("expr:callee:{access_key}:member");
        let access_expression_id = format!("expr:callee:{access_key}");
        let access_value_id = format!("value:callee:{access_key}");
        graph.nodes.push(expression_node(
            &base_expression_id,
            ExpressionKind::Identifier,
            Vec::new(),
            Some(base_name),
            None,
            None,
            None,
            owner.clone(),
        ));
        graph.nodes.push(expression_node(
            &member_expression_id,
            ExpressionKind::Identifier,
            Vec::new(),
            Some(member_name),
            None,
            None,
            None,
            owner.clone(),
        ));
        graph.nodes.push(expression_node(
            &access_expression_id,
            kind,
            vec![base_expression_id, member_expression_id],
            None,
            Some(member_name),
            Some(&format!("{base_name}.{member_name}")),
            Some(&access_value_id),
            owner.clone(),
        ));
        graph.nodes.push(value_node(
            Value {
                value_id: access_value_id.clone(),
                callable_id: Some("callable:callee".to_string()),
                kind: match kind {
                    ExpressionKind::FieldAccess => ValueKind::Field,
                    ExpressionKind::IndexAccess => ValueKind::Index,
                    _ => ValueKind::Unknown,
                },
                role: ValueRole::Unknown,
                symbol_id: None,
                expression_id: Some(access_expression_id),
                call_site_id: None,
                name: Some(member_name.to_string()),
                ordinal: None,
                state_of_value_id: None,
                type_hint: None,
                literal: None,
            },
            owner,
        ));
        access_value_id
    }

    fn expression_node(
        expression_id: &str,
        kind: ExpressionKind,
        child_expression_ids: Vec<String>,
        identifier: Option<&str>,
        member: Option<&str>,
        original_text: Option<&str>,
        value_id: Option<&str>,
        owner: SourceOwnership,
    ) -> GraphNode {
        GraphNode {
            node_id: expression_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::Expression,
            owner,
            span: Some(span(150, 160)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-083 test expression"),
            fact: NodeFact::Expression(sg::Expression {
                expression_id: expression_id.to_string(),
                callable_id: "callable:callee".to_string(),
                statement_id: None,
                parent_expression_id: None,
                kind,
                ordinal: 0,
                child_expression_ids,
                symbol_id: None,
                value_id: value_id.map(str::to_string),
                original_text: original_text.map(str::to_string),
                normalized: sg::NormalizedExpression {
                    canonical: original_text.map(str::to_string),
                    operator: None,
                    identifier: identifier.map(str::to_string),
                    member: member.map(str::to_string),
                    literal: None,
                },
            }),
        }
    }

    fn add_alias_summary_flow(graph: &mut ProgramSupergraph, source_id: &str, target_id: &str) {
        graph.edges.push(GraphEdge {
            edge_id: stable_id("edge", &["sg083-test-alias", source_id, target_id]),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: EdgeKind::DataFlow,
            source_id: source_id.to_string(),
            target_id: Some(target_id.to_string()),
            owner: owner(&graph_artifact_id(graph), "scope:sample", "callable:callee"),
            span: Some(span(170, 180)),
            confidence: Confidence::Unknown,
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-083 test SG-073 alias summary"),
            fact: EdgeFact::DataFlow(sg::DataFlow {
                callable_id: "callable:callee".to_string(),
                name: "value".to_string(),
                flow_kind: DataFlowKind::FieldAccess,
                precision: "sg073-possible-alias-summary".to_string(),
            }),
        });
    }

    fn call_site_node(
        call_site_id: &str,
        callee_expression: &str,
        argument_shape: ArgumentShape,
        owner: SourceOwnership,
    ) -> GraphNode {
        call_site_node_with_dispatch(
            call_site_id,
            callee_expression,
            argument_shape,
            DispatchKind::Direct,
            owner,
        )
    }

    fn call_site_node_with_dispatch(
        call_site_id: &str,
        callee_expression: &str,
        argument_shape: ArgumentShape,
        dispatch_kind: DispatchKind,
        owner: SourceOwnership,
    ) -> GraphNode {
        GraphNode {
            node_id: call_site_id.to_string(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::CallSite,
            owner: owner.clone(),
            span: Some(span(100, 120)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: Vec::new(),
            fact: NodeFact::CallSite(sg::CallSite {
                call_site_id: call_site_id.to_string(),
                artifact_id: owner.artifact_id.clone().expect("artifact owner"),
                enclosing_callable_id: "callable:caller".to_string(),
                span: span(100, 120),
                callee_expression: callee_expression.to_string(),
                argument_shape,
                dispatch_kind,
                context: CallContext::Body,
            }),
        }
    }

    fn value_node(value: Value, owner: SourceOwnership) -> GraphNode {
        GraphNode {
            node_id: value.value_id.clone(),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: NodeKind::Value,
            owner,
            span: Some(span(101, 102)),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: Vec::new(),
            fact: NodeFact::Value(value),
        }
    }

    fn call_edge(
        call_site_id: &str,
        target_id: &str,
        callee_callable_id: Option<String>,
        external_target_id: Option<String>,
        unresolved_target: Option<String>,
        resolution: Resolution,
        owner: SourceOwnership,
    ) -> GraphEdge {
        call_edge_with_kind(
            call_site_id,
            target_id,
            callee_callable_id,
            external_target_id,
            unresolved_target,
            resolution,
            CallEdgeKind::Direct,
            owner,
        )
    }

    fn call_edge_with_kind(
        call_site_id: &str,
        target_id: &str,
        callee_callable_id: Option<String>,
        external_target_id: Option<String>,
        unresolved_target: Option<String>,
        resolution: Resolution,
        kind: CallEdgeKind,
        owner: SourceOwnership,
    ) -> GraphEdge {
        GraphEdge {
            edge_id: stable_id(
                "edge",
                &["calls", call_site_id, target_id, &format!("{resolution:?}")],
            ),
            fact_id: String::new(),
            payload_hash: String::new(),
            kind: EdgeKind::Calls,
            source_id: call_site_id.to_string(),
            target_id: Some(target_id.to_string()),
            owner,
            span: Some(span(100, 120)),
            confidence: match resolution {
                Resolution::Exact | Resolution::External => Confidence::Exact,
                Resolution::Possible | Resolution::Probable | Resolution::Ambiguous => {
                    Confidence::Probable
                }
                Resolution::Unresolved | Resolution::Unsupported => Confidence::Unknown,
            },
            uncertainty: Uncertainty::Exact,
            evidence: inference_evidence("SG-080 test call edge"),
            fact: EdgeFact::Calls(sg::Calls {
                caller_callable_id: "callable:caller".to_string(),
                callee_callable_id,
                external_target_id,
                unresolved_target,
                call_site_id: call_site_id.to_string(),
                kind,
                resolution,
            }),
        }
    }

    fn owner(artifact_id: &str, scope_id: &str, callable_id: &str) -> SourceOwnership {
        SourceOwnership {
            artifact_id: Some(artifact_id.to_string()),
            scope_id: Some(scope_id.to_string()),
            callable_id: Some(callable_id.to_string()),
        }
    }

    fn graph_owner(graph: &ProgramSupergraph) -> SourceOwnership {
        owner(&graph_artifact_id(graph), "scope:sample", "callable:caller")
    }

    fn graph_artifact_id(graph: &ProgramSupergraph) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Artifact(artifact) => Some(artifact.artifact_id.clone()),
                _ => None,
            })
            .expect("artifact node")
    }

    fn parameter_in_edges_for_call<'a>(
        graph: &'a ProgramSupergraph,
        call_site_id: &str,
    ) -> Vec<&'a GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::ParameterIn(parameter) if parameter.call_site_id == call_site_id
                )
            })
            .collect()
    }

    fn returns_to_edges_for_call<'a>(
        graph: &'a ProgramSupergraph,
        call_site_id: &str,
    ) -> Vec<&'a GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::ReturnsTo(returns) if returns.call_site_id == call_site_id
                )
            })
            .collect()
    }

    fn parameter_out_edges_for_call<'a>(
        graph: &'a ProgramSupergraph,
        call_site_id: &str,
    ) -> Vec<&'a GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::ParameterOut(parameter) if parameter.call_site_id == call_site_id
                )
            })
            .collect()
    }

    fn throws_to_edges_for_call<'a>(
        graph: &'a ProgramSupergraph,
        call_site_id: &str,
    ) -> Vec<&'a GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::ThrowsTo(throws) if throws.call_site_id == call_site_id
                )
            })
            .collect()
    }

    fn assert_parameter_in(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        source_id: &str,
        target_id: &str,
        parameter_name: &str,
        confidence: Confidence,
        uncertainty: Uncertainty,
    ) {
        let edge = parameter_in_edges_for_call(graph, call_site_id)
            .into_iter()
            .find(|edge| {
                edge.source_id == source_id && edge.target_id.as_deref() == Some(target_id)
            })
            .unwrap_or_else(|| panic!("missing ParameterIn {source_id} -> {target_id}"));
        assert_eq!(edge.confidence, confidence);
        assert_eq!(edge.uncertainty, uncertainty);
        let EdgeFact::ParameterIn(parameter) = &edge.fact else {
            panic!("expected ParameterIn fact");
        };
        assert_eq!(parameter.parameter_name, parameter_name);
        assert!(parameter.precision.starts_with("sg080"));
    }

    fn assert_receiver_parameter_in(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        source_id: &str,
        target_id: &str,
        parameter_name: &str,
        confidence: Confidence,
        uncertainty: Uncertainty,
        precision: &str,
    ) {
        let edge = parameter_in_edges_for_call(graph, call_site_id)
            .into_iter()
            .find(|edge| {
                edge.source_id == source_id && edge.target_id.as_deref() == Some(target_id)
            })
            .unwrap_or_else(|| panic!("missing receiver ParameterIn {source_id} -> {target_id}"));
        assert_eq!(edge.confidence, confidence);
        assert_eq!(edge.uncertainty, uncertainty);
        let EdgeFact::ParameterIn(parameter) = &edge.fact else {
            panic!("expected ParameterIn fact");
        };
        assert_eq!(parameter.parameter_name, parameter_name);
        assert_eq!(parameter.precision, precision);
    }

    fn assert_returns_to(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        source_id: &str,
        target_id: &str,
        confidence: Confidence,
        uncertainty: Uncertainty,
        precision: &str,
    ) {
        let edge = returns_to_edges_for_call(graph, call_site_id)
            .into_iter()
            .find(|edge| {
                edge.source_id == source_id && edge.target_id.as_deref() == Some(target_id)
            })
            .unwrap_or_else(|| panic!("missing ReturnsTo {source_id} -> {target_id}"));
        assert_eq!(edge.confidence, confidence);
        assert_eq!(edge.uncertainty, uncertainty);
        let EdgeFact::ReturnsTo(returns) = &edge.fact else {
            panic!("expected ReturnsTo fact");
        };
        assert_eq!(returns.precision, precision);
    }

    fn assert_parameter_out(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        source_id: &str,
        target_id: &str,
        parameter_name: &str,
        confidence: Confidence,
        uncertainty: Uncertainty,
        precision: &str,
    ) {
        let edge = parameter_out_edges_for_call(graph, call_site_id)
            .into_iter()
            .find(|edge| {
                edge.source_id == source_id && edge.target_id.as_deref() == Some(target_id)
            })
            .unwrap_or_else(|| panic!("missing ParameterOut {source_id} -> {target_id}"));
        assert_eq!(edge.confidence, confidence);
        assert_eq!(edge.uncertainty, uncertainty);
        let EdgeFact::ParameterOut(parameter) = &edge.fact else {
            panic!("expected ParameterOut fact");
        };
        assert_eq!(parameter.parameter_name, parameter_name);
        assert_eq!(parameter.precision, precision);
    }

    fn assert_throws_to(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        source_id: &str,
        target_id: &str,
        confidence: Confidence,
        uncertainty: Uncertainty,
        target_kind: sg::ThrowsToTargetKind,
        precision: &str,
        exception_type: Option<&str>,
    ) {
        let edge = throws_to_edges_for_call(graph, call_site_id)
            .into_iter()
            .find(|edge| {
                edge.source_id == source_id && edge.target_id.as_deref() == Some(target_id)
            })
            .unwrap_or_else(|| panic!("missing ThrowsTo {source_id} -> {target_id}"));
        assert_eq!(edge.confidence, confidence);
        assert_eq!(edge.uncertainty, uncertainty);
        let EdgeFact::ThrowsTo(throws) = &edge.fact else {
            panic!("expected ThrowsTo fact");
        };
        assert_eq!(throws.target_kind, target_kind);
        assert_eq!(throws.precision, precision);
        assert_eq!(throws.exception_type.as_deref(), exception_type);
    }

    fn assert_parameter_in_endpoints_are_values(graph: &ProgramSupergraph) {
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::ParameterIn)
        {
            assert_eq!(node_kind(graph, &edge.source_id), Some(NodeKind::Value));
            assert_eq!(
                edge.target_id
                    .as_ref()
                    .and_then(|target_id| node_kind(graph, target_id)),
                Some(NodeKind::Value)
            );
        }
    }

    fn assert_returns_to_endpoints_are_values(graph: &ProgramSupergraph) {
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::ReturnsTo)
        {
            assert_eq!(node_kind(graph, &edge.source_id), Some(NodeKind::Value));
            assert_eq!(
                edge.target_id
                    .as_ref()
                    .and_then(|target_id| node_kind(graph, target_id)),
                Some(NodeKind::Value)
            );
        }
    }

    fn assert_diagnostic_for_call(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        expected_kind: DiagnosticKind,
    ) {
        assert!(
            graph.nodes.iter().any(|node| {
                matches!(
                    &node.fact,
                    NodeFact::Diagnostic(diagnostic)
                        if diagnostic.kind == expected_kind
                            && diagnostic.related.contains(&call_site_id.to_string())
                )
            }),
            "missing {expected_kind:?} diagnostic for {call_site_id}"
        );
    }

    fn assert_sg082_diagnostic_for_call(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        expected_kind: DiagnosticKind,
    ) {
        assert!(
            graph.nodes.iter().any(|node| {
                matches!(
                    &node.fact,
                    NodeFact::Diagnostic(diagnostic)
                        if diagnostic.kind == expected_kind
                            && diagnostic.message.contains("SG-082")
                            && diagnostic.related.contains(&call_site_id.to_string())
                )
            }),
            "missing SG-082 {expected_kind:?} diagnostic for {call_site_id}"
        );
    }

    fn assert_sg084_diagnostic_for_call(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        expected_kind: DiagnosticKind,
    ) {
        assert!(
            graph.nodes.iter().any(|node| {
                matches!(
                    &node.fact,
                    NodeFact::Diagnostic(diagnostic)
                        if diagnostic.kind == expected_kind
                            && diagnostic.message.contains("SG-084")
                            && diagnostic.related.contains(&call_site_id.to_string())
                )
            }),
            "missing SG-084 {expected_kind:?} diagnostic for {call_site_id}"
        );
    }

    fn node_kind(graph: &ProgramSupergraph, node_id: &str) -> Option<NodeKind> {
        graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.kind)
    }

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start,
            end_byte: end,
            start_row: 1,
            start_column: start,
            end_row: 1,
            end_column: end,
        }
    }
}
