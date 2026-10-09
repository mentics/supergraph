use crate::intern::Sym;
use std::collections::BTreeSet;

use crate::ast::SourceSpan;

use super::invalidation;
use super::schema::{
    self as sg, CallEdgeKind, Confidence, ControlFlowNodeRole, DataFlowNodeRole, EdgeFact,
    EdgeKind, ExternalTarget, GraphEdge, GraphNode, NodeFact, NodeId, NodeKind, ProgramSupergraph,
    Requirement, SourceSpanIndexKey,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallGraphFilter {
    pub caller_ids: BTreeSet<NodeId>,
    pub callee_ids: BTreeSet<NodeId>,
    pub call_site_ids: BTreeSet<NodeId>,
    pub artifact_ids: BTreeSet<NodeId>,
    pub call_kinds: BTreeSet<CallEdgeKind>,
    pub include_external_targets: bool,
    pub include_unresolved_calls: bool,
    pub min_confidence: Confidence,
}

impl CallGraphFilter {
    pub fn all_calls() -> Self {
        Self {
            caller_ids: BTreeSet::new(),
            callee_ids: BTreeSet::new(),
            call_site_ids: BTreeSet::new(),
            artifact_ids: BTreeSet::new(),
            call_kinds: BTreeSet::new(),
            include_external_targets: true,
            include_unresolved_calls: true,
            min_confidence: Confidence::Unknown,
        }
    }

    pub fn local_calls() -> Self {
        Self {
            include_external_targets: false,
            include_unresolved_calls: false,
            ..Self::all_calls()
        }
    }

    pub fn with_caller(mut self, caller_id: impl Into<NodeId>) -> Self {
        self.caller_ids.insert(caller_id.into());
        self
    }

    pub fn with_callee(mut self, callee_id: impl Into<NodeId>) -> Self {
        self.callee_ids.insert(callee_id.into());
        self
    }

    pub fn with_call_site(mut self, call_site_id: impl Into<NodeId>) -> Self {
        self.call_site_ids.insert(call_site_id.into());
        self
    }

    pub fn with_artifact(mut self, artifact_id: impl Into<NodeId>) -> Self {
        self.artifact_ids.insert(artifact_id.into());
        self
    }

    pub fn with_call_kind(mut self, kind: CallEdgeKind) -> Self {
        self.call_kinds.insert(kind);
        self
    }

    pub fn with_min_confidence(mut self, confidence: Confidence) -> Self {
        self.min_confidence = confidence;
        self
    }

    pub fn matches_edge(&self, edge: &GraphEdge) -> bool {
        let EdgeFact::Calls(calls) = &edge.fact else {
            return false;
        };

        if edge.confidence < self.min_confidence {
            return false;
        }
        if !self.caller_ids.is_empty() && !self.caller_ids.contains(&calls.caller_callable_id) {
            return false;
        }
        if !self.call_site_ids.is_empty() && !self.call_site_ids.contains(&calls.call_site_id) {
            return false;
        }
        if !self.call_kinds.is_empty() && !self.call_kinds.contains(&calls.kind) {
            return false;
        }
        if !self.artifact_ids.is_empty()
            && edge
                .owner
                .artifact_id
                .as_ref()
                .is_none_or(|artifact_id| !self.artifact_ids.contains(artifact_id))
        {
            return false;
        }
        if calls.external_target_id.is_some() && !self.include_external_targets {
            return false;
        }
        if calls.unresolved_target.is_some() && !self.include_unresolved_calls {
            return false;
        }
        if !self.callee_ids.is_empty()
            && edge
                .target_id
                .as_ref()
                .is_none_or(|target_id| !self.callee_ids.contains(target_id))
        {
            return false;
        }

        true
    }
}

impl Default for CallGraphFilter {
    fn default() -> Self {
        Self::all_calls()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CallGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

impl<'a> CallGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn call_edges(
        &'a self,
        filter: &'a CallGraphFilter,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        let edge_ids = indexed_call_edge_ids(self.graph, filter);
        edge_ids
            .into_iter()
            .filter_map(move |edge_id| indexed_edge(self.graph, edge_id))
            .filter(move |edge| filter.matches_edge(edge))
    }

    pub fn calls_from_call_site(&'a self, call_site_id: NodeId) -> Vec<&'a GraphEdge> {
        self.graph
            .indexes
            .call_site_to_calls
            .get(&call_site_id)
            .into_iter()
            .flatten()
            .filter_map(|edge_id| indexed_edge(self.graph, *edge_id))
            .collect()
    }

    pub fn calls_from_caller_to_callee(
        &'a self,
        caller_id: NodeId,
        callee_id: NodeId,
    ) -> Vec<&'a GraphEdge> {
        self.calls_from_caller_to_target(caller_id, callee_id)
    }

    pub fn calls_from_caller_to_target(
        &'a self,
        caller_id: NodeId,
        target_id: NodeId,
    ) -> Vec<&'a GraphEdge> {
        if let Some(edge_ids) = self
            .graph
            .indexes
            .caller_to_concrete_target_calls
            .get(&caller_id)
            .and_then(|targets| targets.get(&target_id))
        {
            return edge_ids
                .iter()
                .filter_map(|edge_id| indexed_edge(self.graph, *edge_id))
                .collect();
        }

        let filter = CallGraphFilter::all_calls()
            .with_caller(caller_id)
            .with_callee(target_id);
        indexed_call_edge_ids(self.graph, &filter)
            .into_iter()
            .filter_map(|edge_id| indexed_edge(self.graph, edge_id))
            .filter(|edge| filter.matches_edge(edge))
            .collect()
    }

    pub fn call_targets_from_caller(&'a self, caller_id: NodeId) -> Vec<NodeId> {
        self.graph
            .indexes
            .caller_to_concrete_call_targets
            .get(&caller_id)
            .into_iter()
            .flatten()
            .copied()
            .collect()
    }

    pub fn callables(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_nodes_by_kind(self.graph, sg::NodeKind::Callable).into_iter()
    }

    pub fn call_sites(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_nodes_by_kind(self.graph, sg::NodeKind::CallSite).into_iter()
    }

    pub fn external_targets(&'a self) -> impl Iterator<Item = &'a ExternalTarget> + 'a {
        indexed_nodes_by_kind(self.graph, sg::NodeKind::ExternalTarget)
            .into_iter()
            .filter_map(|node| match &node.fact {
                NodeFact::ExternalTarget(target) => Some(target),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StructuralGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

impl<'a> StructuralGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn node(&'a self, node_id: NodeId) -> Option<&'a GraphNode> {
        indexed_node(self.graph, node_id)
    }

    pub fn edge(&'a self, edge_id: sg::EdgeId) -> Option<&'a GraphEdge> {
        indexed_edge(self.graph, edge_id)
    }

    pub fn nodes_by_kind(&'a self, kind: NodeKind) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_nodes_by_kind(self.graph, kind).into_iter()
    }

    pub fn edges_by_kind(&'a self, kind: EdgeKind) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_edges_by_kinds(self.graph, &[kind]).into_iter()
    }

    pub fn artifacts(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_by_kind(NodeKind::Artifact)
    }

    pub fn scopes(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_by_kind(NodeKind::Scope)
    }

    pub fn callables(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_by_kind(NodeKind::Callable)
    }

    pub fn statements(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_by_kind(NodeKind::Statement)
    }

    pub fn expressions(&'a self) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_by_kind(NodeKind::Expression)
    }

    pub fn contains_children(
        &'a self,
        container_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_outgoing_edges_by_kind(self.graph, container_id, EdgeKind::Contains)
            .into_iter()
            .filter_map(move |edge| edge.target_id)
            .filter_map(move |node_id| indexed_node(self.graph, node_id))
    }

    pub fn contains_parent(&'a self, member_id: NodeId) -> Option<&'a GraphNode> {
        indexed_incoming_edges_by_kind(self.graph, member_id, EdgeKind::Contains)
            .into_iter()
            .find_map(|edge| indexed_node(self.graph, edge.source_id))
    }

    pub fn nodes_at_source_span(
        &'a self,
        artifact_id: Option<NodeId>,
        span: SourceSpan,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        let key = SourceSpanIndexKey {
            artifact_id: artifact_id,
            span,
        };
        self.graph
            .indexes
            .source_span_to_nodes
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(move |node_id| indexed_node(self.graph, *node_id))
    }

    pub fn nodes_owned_by_artifact(
        &'a self,
        artifact_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.graph
            .indexes
            .artifact_to_nodes
            .get(&artifact_id)
            .into_iter()
            .flatten()
            .filter_map(move |node_id| indexed_node(self.graph, *node_id))
    }

    pub fn nodes_owned_by_callable(
        &'a self,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.graph
            .indexes
            .callable_to_nodes
            .get(&callable_id)
            .into_iter()
            .flatten()
            .filter_map(move |node_id| indexed_node(self.graph, *node_id))
    }

    pub fn edges_owned_by(&'a self, owner_id: NodeId) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        self.graph
            .indexes
            .owner_to_edges
            .get(&owner_id)
            .into_iter()
            .flatten()
            .filter_map(move |edge_id| indexed_edge(self.graph, *edge_id))
    }

    pub fn definitions_for_symbol(
        &'a self,
        symbol_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.graph
            .indexes
            .symbol_to_definitions
            .get(&symbol_id)
            .into_iter()
            .flatten()
            .filter_map(move |node_id| indexed_node(self.graph, *node_id))
    }

    pub fn uses_for_symbol(
        &'a self,
        symbol_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.graph
            .indexes
            .symbol_to_uses
            .get(&symbol_id)
            .into_iter()
            .flatten()
            .filter_map(move |node_id| indexed_node(self.graph, *node_id))
    }
}

impl ProgramSupergraph {
    pub fn node(&self, node_id: NodeId) -> Option<&GraphNode> {
        indexed_node(self, node_id)
    }

    pub fn edge(&self, edge_id: sg::EdgeId) -> Option<&GraphEdge> {
        indexed_edge(self, edge_id)
    }

    pub fn structural_view(&self) -> StructuralGraphView<'_> {
        StructuralGraphView::new(self)
    }

    pub fn call_graph_view(&self) -> CallGraphView<'_> {
        CallGraphView::new(self)
    }

    pub fn control_flow_view(&self) -> ControlFlowGraphView<'_> {
        ControlFlowGraphView::new(self)
    }

    pub fn data_flow_view(&self) -> DataFlowGraphView<'_> {
        DataFlowGraphView::new(self)
    }

    pub fn program_dependence_view(&self) -> ProgramDependenceGraphView<'_> {
        ProgramDependenceGraphView::new(self)
    }

    pub fn system_dependence_view(&self) -> SystemDependenceGraphView<'_> {
        SystemDependenceGraphView::new(self)
    }


    pub fn traceability_view(&self) -> TraceabilityGraphView<'_> {
        TraceabilityGraphView::new(self)
    }

    pub fn invalidation_view(&self) -> InvalidationGraphView<'_> {
        InvalidationGraphView::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ControlFlowGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

impl<'a> ControlFlowGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn nodes_for_callable(
        &'a self,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_nodes_by_kind(self.graph, NodeKind::ControlFlow)
            .into_iter()
            .filter(move |node| {
                matches!(&node.fact, NodeFact::ControlFlow(control) if control.callable_id == callable_id)
            })
    }

    pub fn edges_for_callable(
        &'a self,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_edges_by_kinds(self.graph, &[EdgeKind::ControlFlow])
            .into_iter()
            .filter(move |edge| {
                matches!(&edge.fact, EdgeFact::ControlFlow(flow) if flow.callable_id == callable_id)
            })
    }

    pub fn successors(&'a self, cfg_node_id: NodeId) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_outgoing_edges_by_kind(self.graph, cfg_node_id, EdgeKind::ControlFlow).into_iter()
    }

    pub fn predecessors(
        &'a self,
        cfg_node_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_incoming_edges_by_kind(self.graph, cfg_node_id, EdgeKind::ControlFlow).into_iter()
    }

    pub fn entry_nodes(&'a self, callable_id: NodeId) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_for_callable(callable_id).filter(|node| {
            matches!(
                &node.fact,
                NodeFact::ControlFlow(control) if control.role == ControlFlowNodeRole::Entry
            )
        })
    }

    pub fn exit_nodes(&'a self, callable_id: NodeId) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.nodes_for_callable(callable_id).filter(|node| {
            matches!(
                &node.fact,
                NodeFact::ControlFlow(control) if control.role == ControlFlowNodeRole::Exit
            )
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DataFlowGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

impl<'a> DataFlowGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn values_for_callable(
        &'a self,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_nodes_by_kind(self.graph, NodeKind::Value)
            .into_iter()
            .filter(move |node| {
                matches!(&node.fact, NodeFact::Value(value) if value.callable_id == Some(callable_id))
            })
    }

    pub fn values_for_call_site(
        &'a self,
        call_site_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        indexed_nodes_by_kind(self.graph, NodeKind::Value)
            .into_iter()
            .filter(move |node| {
                matches!(&node.fact, NodeFact::Value(value) if value.call_site_id == Some(call_site_id))
            })
    }

    pub fn edges_for_callable(
        &'a self,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_edges_by_kinds(self.graph, dfg_edge_kinds())
            .into_iter()
            .filter(move |edge| edge_mentions_callable(edge, callable_id))
    }

    pub fn outgoing_value_edges(
        &'a self,
        value_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        dfg_value_edge_kinds()
            .iter()
            .flat_map(move |kind| indexed_outgoing_edges_by_kind(self.graph, value_id, *kind))
            .filter(|edge| edge.target_id.is_some())
    }

    pub fn incoming_value_edges(
        &'a self,
        value_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        dfg_value_edge_kinds()
            .iter()
            .flat_map(move |kind| indexed_incoming_edges_by_kind(self.graph, value_id, *kind))
    }

    pub fn value_forward_slice(&self, seed_value_id: NodeId) -> ValueForwardSlice {
        ProgramDependenceGraphView::new(self.graph).value_forward_slice(seed_value_id)
    }

    pub fn value_backward_slice(&self, seed_value_id: NodeId) -> ValueForwardSlice {
        let mut value_ids = BTreeSet::new();
        let mut edge_ids = BTreeSet::new();
        let mut frontier = vec![seed_value_id];

        while let Some(value_id) = frontier.pop() {
            if !value_ids.insert(value_id) {
                continue;
            }

            for edge in self.incoming_value_edges(value_id) {
                edge_ids.insert(edge.edge_id);
                if is_value_node(self.graph, edge.source_id)
                    && !value_ids.contains(&edge.source_id)
                {
                    frontier.push(edge.source_id);
                }
            }
        }

        let diagnostic_ids = diagnostic_ids_for_slice(self.graph, &value_ids, &edge_ids);
        ValueForwardSlice {
            seed_value_id: seed_value_id,
            value_ids: value_ids.into_iter().collect(),
            edge_ids: edge_ids.into_iter().collect(),
            diagnostic_ids,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TraceabilityGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

impl<'a> TraceabilityGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn requirement_to_code_edges(
        &'a self,
        requirement_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_outgoing_edges_by_kind(self.graph, requirement_id, EdgeKind::TracesTo).into_iter()
    }

    pub fn code_to_requirement_edges(
        &'a self,
        code_fact_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_incoming_edges_by_kind(self.graph, code_fact_id, EdgeKind::TracesTo).into_iter()
    }

    pub fn code_facts_for_requirement(
        &'a self,
        requirement_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.requirement_to_code_edges(requirement_id)
            .filter_map(move |edge| edge.target_id)
            .filter_map(move |node_id| indexed_node(self.graph, node_id))
    }

    pub fn requirements_for_code(
        &'a self,
        code_fact_id: NodeId,
    ) -> impl Iterator<Item = &'a Requirement> + 'a {
        self.graph
            .indexes
            .code_to_requirements
            .get(&code_fact_id)
            .into_iter()
            .flatten()
            .filter_map(move |requirement_id| indexed_node(self.graph, *requirement_id))
            .filter_map(|node| match &node.fact {
                NodeFact::Requirement(requirement) => Some(requirement),
                _ => None,
            })
    }

    pub fn requirement_to_domain_knowledge_edges(
        &'a self,
        requirement_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_outgoing_edges_by_kind(
            self.graph,
            requirement_id,
            EdgeKind::DependsOnDomainKnowledge,
        )
        .into_iter()
    }

    pub fn domain_knowledge_to_requirement_edges(
        &'a self,
        domain_knowledge_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_incoming_edges_by_kind(
            self.graph,
            domain_knowledge_id,
            EdgeKind::DependsOnDomainKnowledge,
        )
        .into_iter()
    }

    pub fn domain_knowledge_for_requirement(
        &'a self,
        requirement_id: NodeId,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        self.requirement_to_domain_knowledge_edges(requirement_id)
            .filter_map(move |edge| edge.target_id)
            .filter_map(move |node_id| indexed_node(self.graph, node_id))
            .filter(|node| matches!(&node.fact, NodeFact::DomainKnowledge(_)))
    }

    pub fn requirements_for_domain_knowledge(
        &'a self,
        domain_knowledge_id: NodeId,
    ) -> impl Iterator<Item = &'a Requirement> + 'a {
        self.graph
            .indexes
            .domain_knowledge_to_requirements
            .get(&domain_knowledge_id)
            .into_iter()
            .flatten()
            .filter_map(move |requirement_id| indexed_node(self.graph, *requirement_id))
            .filter_map(|node| match &node.fact {
                NodeFact::Requirement(requirement) => Some(requirement),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InvalidationGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

impl<'a> InvalidationGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn invalidated_by(
        &'a self,
        invalidator_id: impl Into<invalidation::InvalidationSubject>,
    ) -> Vec<invalidation::InvalidationDependency> {
        invalidation::invalidated_by(self.graph, invalidator_id)
    }

    pub fn invalidation_closure(
        &'a self,
        invalidator_id: impl Into<invalidation::InvalidationSubject>,
    ) -> BTreeSet<invalidation::InvalidationSubject> {
        invalidation::invalidation_closure(self.graph, invalidator_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramDependenceGraphFilter {
    pub callable_ids: BTreeSet<NodeId>,
    pub min_confidence: Confidence,
}

impl ProgramDependenceGraphFilter {
    pub fn all_callables() -> Self {
        Self {
            callable_ids: BTreeSet::new(),
            min_confidence: Confidence::Unknown,
        }
    }

    pub fn with_callable(mut self, callable_id: impl Into<NodeId>) -> Self {
        self.callable_ids.insert(callable_id.into());
        self
    }

    pub fn with_min_confidence(mut self, confidence: Confidence) -> Self {
        self.min_confidence = confidence;
        self
    }

    pub fn matches_edge(&self, edge: &GraphEdge) -> bool {
        if edge.confidence < self.min_confidence {
            return false;
        }
        let callable_id = match &edge.fact {
            EdgeFact::Controls(controls) => &controls.callable_id,
            EdgeFact::DataFlow(data_flow) => &data_flow.callable_id,
            _ => return false,
        };
        self.callable_ids.is_empty() || self.callable_ids.contains(callable_id)
    }
}

impl Default for ProgramDependenceGraphFilter {
    fn default() -> Self {
        Self::all_callables()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramDependenceGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueForwardSlice {
    pub seed_value_id: NodeId,
    pub value_ids: Vec<NodeId>,
    pub edge_ids: Vec<sg::EdgeId>,
    pub diagnostic_ids: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorBackwardSlice {
    pub behavior_id: NodeId,
    pub value_ids: Vec<NodeId>,
    pub control_condition_ids: Vec<NodeId>,
    pub path_conditions: Vec<sg::PathConditionSummary>,
    pub edge_ids: Vec<sg::EdgeId>,
    pub diagnostic_ids: Vec<NodeId>,
}

impl<'a> ProgramDependenceGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn edges(
        &'a self,
        filter: &'a ProgramDependenceGraphFilter,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_edges_by_kinds(self.graph, &[EdgeKind::Controls, EdgeKind::DataFlow])
            .into_iter()
            .filter(move |edge| filter.matches_edge(edge))
    }

    pub fn nodes(
        &'a self,
        filter: &'a ProgramDependenceGraphFilter,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        let node_ids = self
            .edges(filter)
            .flat_map(|edge| std::iter::once(edge.source_id).chain(edge.target_id))
            .collect::<BTreeSet<_>>();
        node_ids
            .into_iter()
            .filter_map(move |node_id| indexed_node(self.graph, node_id))
    }

    pub fn value_forward_slice(&self, seed_value_id: NodeId) -> ValueForwardSlice {
        let mut value_ids = BTreeSet::new();
        let mut edge_ids = BTreeSet::new();
        let mut frontier = vec![seed_value_id];

        while let Some(value_id) = frontier.pop() {
            if !value_ids.insert(value_id) {
                continue;
            }

            for edge in self.outgoing_value_flow_edges(value_id) {
                let Some(target_id) = edge.target_id else {
                    continue;
                };
                if !is_value_node(self.graph, target_id) {
                    continue;
                }
                edge_ids.insert(edge.edge_id);
                if !value_ids.contains(&target_id) {
                    frontier.push(target_id);
                }
            }
        }

        let diagnostic_ids = diagnostic_ids_for_slice(self.graph, &value_ids, &edge_ids);
        ValueForwardSlice {
            seed_value_id: seed_value_id,
            value_ids: value_ids.into_iter().collect(),
            edge_ids: edge_ids.into_iter().collect(),
            diagnostic_ids,
        }
    }

    pub fn behavior_backward_slice(&self, behavior_id: NodeId) -> BehaviorBackwardSlice {
        let mut value_ids = seed_values_for_behavior(self.graph, behavior_id);
        let mut edge_ids = BTreeSet::new();
        let mut frontier = value_ids.iter().copied().collect::<Vec<_>>();

        while let Some(value_id) = frontier.pop() {
            for edge in self.incoming_value_flow_edges(value_id) {
                edge_ids.insert(edge.edge_id);
                if is_value_node(self.graph, edge.source_id)
                    && value_ids.insert(edge.source_id)
                {
                    frontier.push(edge.source_id);
                }
            }
        }

        let controls = incoming_control_edges(self.graph, behavior_id);
        let control_condition_ids = controls
            .iter()
            .map(|edge| {
                edge_ids.insert(edge.edge_id);
                edge.source_id
            })
            .collect::<BTreeSet<_>>();
        let path_conditions = path_conditions_for_behavior(self.graph, behavior_id, &controls);
        let diagnostic_ids = diagnostic_ids_for_slice(self.graph, &value_ids, &edge_ids);

        BehaviorBackwardSlice {
            behavior_id: behavior_id,
            value_ids: value_ids.into_iter().collect(),
            control_condition_ids: control_condition_ids.into_iter().collect(),
            path_conditions,
            edge_ids: edge_ids.into_iter().collect(),
            diagnostic_ids,
        }
    }

    pub fn return_backward_slice(&self, return_id: NodeId) -> BehaviorBackwardSlice {
        self.behavior_backward_slice(return_id)
    }

    pub fn write_backward_slice(&self, write_id: NodeId) -> BehaviorBackwardSlice {
        self.behavior_backward_slice(write_id)
    }

    pub fn call_backward_slice(&self, call_site_id: NodeId) -> BehaviorBackwardSlice {
        self.behavior_backward_slice(call_site_id)
    }

    pub fn branch_backward_slice(&self, condition_id: NodeId) -> BehaviorBackwardSlice {
        self.behavior_backward_slice(condition_id)
    }

    fn outgoing_value_flow_edges(&self, value_id: NodeId) -> Vec<&'a GraphEdge> {
        [EdgeKind::DataFlow, EdgeKind::ReturnsTo, EdgeKind::ThrowsTo]
            .into_iter()
            .flat_map(|kind| indexed_outgoing_edges_by_kind(self.graph, value_id, kind))
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::DataFlow(_) | EdgeFact::ReturnsTo(_) | EdgeFact::ThrowsTo(_)
                )
            })
            .collect()
    }

    fn incoming_value_flow_edges(&self, value_id: NodeId) -> Vec<&'a GraphEdge> {
        [EdgeKind::DataFlow, EdgeKind::ReturnsTo, EdgeKind::ThrowsTo]
            .into_iter()
            .flat_map(|kind| indexed_incoming_edges_by_kind(self.graph, value_id, kind))
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::DataFlow(_) | EdgeFact::ReturnsTo(_) | EdgeFact::ThrowsTo(_)
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemDependenceGraphFilter {
    pub callable_ids: BTreeSet<NodeId>,
    pub min_confidence: Confidence,
}

impl SystemDependenceGraphFilter {
    pub fn all_callables() -> Self {
        Self {
            callable_ids: BTreeSet::new(),
            min_confidence: Confidence::Unknown,
        }
    }

    pub fn with_callable(mut self, callable_id: impl Into<NodeId>) -> Self {
        self.callable_ids.insert(callable_id.into());
        self
    }

    pub fn with_min_confidence(mut self, confidence: Confidence) -> Self {
        self.min_confidence = confidence;
        self
    }

    pub fn matches_edge(&self, edge: &GraphEdge) -> bool {
        if edge.confidence < self.min_confidence {
            return false;
        }
        let endpoints = match &edge.fact {
            EdgeFact::Controls(controls) => {
                Some((controls.callable_id, Some(controls.callable_id)))
            }
            EdgeFact::DataFlow(data_flow) => {
                Some((data_flow.callable_id, Some(data_flow.callable_id)))
            }
            EdgeFact::Calls(calls) => Some((
                calls.caller_callable_id,
                calls.callee_callable_id.or(calls.external_target_id),
            )),
            EdgeFact::ParameterIn(parameter) => Some((
                parameter.caller_callable_id,
                Some(parameter.callee_callable_id),
            )),
            EdgeFact::ReturnsTo(returns) => Some((
                returns.callee_callable_id,
                Some(returns.caller_callable_id),
            )),
            EdgeFact::ParameterOut(parameter) => Some((
                parameter.callee_callable_id,
                Some(parameter.caller_callable_id),
            )),
            EdgeFact::ThrowsTo(throws) => Some((
                throws.callee_callable_id,
                Some(throws.caller_callable_id),
            )),
            _ => None,
        };
        let Some((left, right)) = endpoints else {
            return false;
        };
        self.callable_ids.is_empty()
            || self.callable_ids.contains(&left)
            || right.is_some_and(|right| self.callable_ids.contains(&right))
    }
}

impl Default for SystemDependenceGraphFilter {
    fn default() -> Self {
        Self::all_callables()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemDependenceGraphView<'a> {
    graph: &'a ProgramSupergraph,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemDependenceSlice {
    pub seed_id: NodeId,
    pub node_ids: Vec<NodeId>,
    pub value_ids: Vec<NodeId>,
    pub callable_ids: Vec<NodeId>,
    pub requirement_ids: Vec<NodeId>,
    pub edge_ids: Vec<sg::EdgeId>,
    pub diagnostic_ids: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdgSink {
    pub sink_id: NodeId,
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub edge_id: sg::EdgeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrypointSinkPath {
    pub entrypoint_id: NodeId,
    pub sink_id: NodeId,
    pub callable_ids: Vec<NodeId>,
    pub call_site_ids: Vec<NodeId>,
    pub edge_ids: Vec<sg::EdgeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalleeImpactSlice {
    pub callee_id: NodeId,
    pub affected_caller_ids: Vec<NodeId>,
    pub affected_call_site_ids: Vec<NodeId>,
    pub edge_ids: Vec<sg::EdgeId>,
    pub requirement_ids: Vec<NodeId>,
    pub diagnostic_ids: Vec<NodeId>,
}

impl<'a> SystemDependenceGraphView<'a> {
    pub fn new(graph: &'a ProgramSupergraph) -> Self {
        Self { graph }
    }

    pub fn edges(
        &'a self,
        filter: &'a SystemDependenceGraphFilter,
    ) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        indexed_edges_by_kinds(
            self.graph,
            &[
                EdgeKind::Controls,
                EdgeKind::DataFlow,
                EdgeKind::Calls,
                EdgeKind::ParameterIn,
                EdgeKind::ReturnsTo,
                EdgeKind::ParameterOut,
                EdgeKind::ThrowsTo,
            ],
        )
        .into_iter()
        .filter(move |edge| filter.matches_edge(edge))
    }

    pub fn nodes(
        &'a self,
        filter: &'a SystemDependenceGraphFilter,
    ) -> impl Iterator<Item = &'a GraphNode> + 'a {
        let node_ids = self
            .edges(filter)
            .flat_map(|edge| std::iter::once(edge.source_id).chain(edge.target_id))
            .collect::<BTreeSet<_>>();
        node_ids
            .into_iter()
            .filter_map(move |node_id| indexed_node(self.graph, node_id))
    }

    pub fn value_forward_slice(&self, seed_value_id: NodeId) -> SystemDependenceSlice {
        system_slice(
            self.graph,
            seed_value_id,
            SdgTraversalDirection::Forward,
            false,
        )
    }

    pub fn value_backward_slice(&self, seed_value_id: NodeId) -> SystemDependenceSlice {
        system_slice(
            self.graph,
            seed_value_id,
            SdgTraversalDirection::Backward,
            false,
        )
    }

    pub fn cross_call_slice(&self, seed_id: NodeId) -> SystemDependenceSlice {
        system_slice(self.graph, seed_id, SdgTraversalDirection::Both, false)
    }

    pub fn requirement_slice(&self, requirement_id: NodeId) -> SystemDependenceSlice {
        system_slice(
            self.graph,
            requirement_id,
            SdgTraversalDirection::Both,
            true,
        )
    }

    pub fn entrypoint_candidates(&self) -> Vec<NodeId> {
        let mut called_local_targets = BTreeSet::new();
        for edge in indexed_edges_by_kinds(self.graph, &[EdgeKind::Calls]) {
            if let EdgeFact::Calls(calls) = &edge.fact
                && let Some(callee_id) = calls.callee_callable_id
            {
                called_local_targets.insert(callee_id);
            }
        }

        indexed_nodes_by_kind(self.graph, sg::NodeKind::Callable)
            .into_iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Callable(callable)
                    if callable.kind == sg::CallableKind::ModuleInitializer
                        || !callable.external_invocation_metadata.is_empty()
                        || !called_local_targets.contains(&callable.callable_id) =>
                {
                    Some(callable.callable_id)
                }
                _ => None,
            })
            .collect()
    }

    pub fn sink_candidates(&self) -> Vec<SdgSink> {
        indexed_edges_by_kinds(self.graph, &[EdgeKind::Calls])
            .into_iter()
            .filter_map(|edge| match &edge.fact {
                EdgeFact::Calls(calls)
                    if calls.external_target_id.is_some() || calls.unresolved_target.is_some() =>
                {
                    Some(SdgSink {
                        sink_id: edge
                            .target_id
                            .or(calls.external_target_id)
                            .unwrap_or_else(|| calls.call_site_id),
                        call_site_id: calls.call_site_id,
                        caller_callable_id: calls.caller_callable_id,
                        edge_id: edge.edge_id,
                    })
                }
                _ => None,
            })
            .collect()
    }

    pub fn entrypoint_to_sink_slice(
        &self,
        entrypoint_id: NodeId,
        sink_id: NodeId,
    ) -> Option<EntrypointSinkPath> {
        let mut visited = BTreeSet::new();
        let mut frontier = vec![(
            entrypoint_id,
            vec![entrypoint_id],
            Vec::<NodeId>::new(),
            Vec::<sg::EdgeId>::new(),
        )];

        while let Some((callable_id, callable_path, call_site_path, edge_path)) = frontier.pop() {
            if !visited.insert(callable_id) {
                continue;
            }

            for edge in self.calls_from_callable(callable_id) {
                let EdgeFact::Calls(calls) = &edge.fact else {
                    continue;
                };
                let mut next_call_sites = call_site_path.clone();
                next_call_sites.push(calls.call_site_id);
                let mut next_edges = edge_path.clone();
                next_edges.push(edge.edge_id);
                let target_id = edge
                    .target_id
                    .or(calls.external_target_id);
                if target_id == Some(sink_id) || calls.call_site_id == sink_id {
                    return Some(EntrypointSinkPath {
                        entrypoint_id: entrypoint_id,
                        sink_id: sink_id,
                        callable_ids: callable_path,
                        call_site_ids: next_call_sites,
                        edge_ids: next_edges,
                    });
                }
                if let Some(callee_id) = calls.callee_callable_id
                    && indexed_node(self.graph, callee_id)
                        .is_some_and(|node| matches!(&node.fact, NodeFact::Callable(_)))
                {
                    let mut next_callables = callable_path.clone();
                    next_callables.push(callee_id);
                    frontier.push((
                        callee_id,
                        next_callables,
                        next_call_sites,
                        next_edges,
                    ));
                }
            }
        }

        None
    }

    pub fn callers_affected_by_callee(&self, callee_id: NodeId) -> CalleeImpactSlice {
        let mut affected_caller_ids = BTreeSet::new();
        let mut affected_call_site_ids = BTreeSet::new();
        let mut edge_ids = BTreeSet::new();
        let mut frontier = vec![callee_id];
        let mut visited_callees = BTreeSet::new();

        while let Some(current_callee_id) = frontier.pop() {
            if !visited_callees.insert(current_callee_id) {
                continue;
            }

            for edge in self.incoming_call_and_summary_edges(current_callee_id) {
                let Some((caller_id, call_site_id)) = caller_and_call_site_for_impact(edge) else {
                    continue;
                };
                edge_ids.insert(edge.edge_id);
                affected_call_site_ids.insert(call_site_id);
                if affected_caller_ids.insert(caller_id) {
                    frontier.push(caller_id);
                }
            }
        }

        let requirement_ids = requirements_for_edges(self.graph, &edge_ids);
        let value_ids = BTreeSet::new();
        let diagnostic_ids = diagnostic_ids_for_slice(self.graph, &value_ids, &edge_ids);
        CalleeImpactSlice {
            callee_id: callee_id,
            affected_caller_ids: affected_caller_ids.into_iter().collect(),
            affected_call_site_ids: affected_call_site_ids.into_iter().collect(),
            edge_ids: edge_ids.into_iter().collect(),
            requirement_ids,
            diagnostic_ids,
        }
    }

    fn calls_from_callable(&self, callable_id: NodeId) -> Vec<&'a GraphEdge> {
        self.graph
            .indexes
            .calls_by_caller
            .get(&callable_id)
            .into_iter()
            .flatten()
            .filter_map(|edge_id| indexed_edge(self.graph, *edge_id))
            .collect()
    }

    fn incoming_call_and_summary_edges(&self, callee_id: NodeId) -> Vec<&'a GraphEdge> {
        sdg_edge_kinds()
            .iter()
            .flat_map(|kind| indexed_edges_by_kinds(self.graph, &[*kind]))
            .filter(|edge| match &edge.fact {
                EdgeFact::Calls(calls) => calls.callee_callable_id == Some(callee_id),
                EdgeFact::ParameterIn(parameter) => parameter.callee_callable_id == callee_id,
                EdgeFact::ReturnsTo(returns) => returns.callee_callable_id == callee_id,
                EdgeFact::ParameterOut(parameter) => parameter.callee_callable_id == callee_id,
                EdgeFact::ThrowsTo(throws) => throws.callee_callable_id == callee_id,
                _ => false,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdgTraversalDirection {
    Forward,
    Backward,
    Both,
}

pub fn system_slice(
    graph: &ProgramSupergraph,
    seed_id: NodeId,
    direction: SdgTraversalDirection,
    include_requirements: bool,
) -> SystemDependenceSlice {
    let mut node_ids = BTreeSet::new();
    let mut edge_ids = BTreeSet::new();
    let mut frontier = vec![seed_id];

    while let Some(node_id) = frontier.pop() {
        if !node_ids.insert(node_id) {
            continue;
        }

        for value_id in seed_values_for_behavior(graph, node_id) {
            if !node_ids.contains(&value_id) {
                frontier.push(value_id);
            }
        }
        if include_requirements {
            for related_id in related_code_ids_for_value(graph, node_id) {
                if !node_ids.contains(&related_id) {
                    frontier.push(related_id);
                }
            }
        }

        for edge in traversal_edges(graph, node_id, direction, include_requirements) {
            edge_ids.insert(edge.edge_id);
            let endpoint_id = if edge.source_id == node_id {
                edge.target_id
            } else {
                Some(edge.source_id)
            };
            if let Some(endpoint_id) = endpoint_id
                && !node_ids.contains(&endpoint_id)
            {
                frontier.push(endpoint_id);
            }
        }

        if include_requirements {
            for requirement_id in requirements_for_code_or_related_value(graph, node_id) {
                if !node_ids.contains(&requirement_id) {
                    frontier.push(requirement_id);
                }
            }
        }
    }

    let value_ids = node_ids
        .iter()
        .filter(|node_id| is_value_node(graph, **node_id))
        .copied()
        .collect::<BTreeSet<_>>();
    let requirement_ids = node_ids
        .iter()
        .filter(|node_id| {
            indexed_node(graph, **node_id)
                .is_some_and(|node| matches!(&node.fact, NodeFact::Requirement(_)))
        })
        .copied()
        .collect::<Vec<_>>();
    let callable_ids = callable_ids_for_slice(graph, &node_ids, &edge_ids);
    let diagnostic_ids = diagnostic_ids_for_slice(graph, &value_ids, &edge_ids);

    SystemDependenceSlice {
        seed_id: seed_id,
        node_ids: node_ids.into_iter().collect(),
        value_ids: value_ids.into_iter().collect(),
        callable_ids,
        requirement_ids,
        edge_ids: edge_ids.into_iter().collect(),
        diagnostic_ids,
    }
}

pub fn traversal_edges<'a>(
    graph: &'a ProgramSupergraph,
    node_id: NodeId,
    direction: SdgTraversalDirection,
    include_requirements: bool,
) -> Vec<&'a GraphEdge> {
    let mut edges = Vec::new();
    if matches!(
        direction,
        SdgTraversalDirection::Forward | SdgTraversalDirection::Both
    ) {
        edges.extend(
            sdg_edge_kinds()
                .iter()
                .flat_map(|kind| indexed_outgoing_edges_by_kind(graph, node_id, *kind)),
        );
        if include_requirements {
            edges.extend(
                requirement_edge_kinds()
                    .iter()
                    .flat_map(|kind| indexed_outgoing_edges_by_kind(graph, node_id, *kind)),
            );
        }
    }
    if matches!(
        direction,
        SdgTraversalDirection::Backward | SdgTraversalDirection::Both
    ) {
        edges.extend(
            sdg_edge_kinds()
                .iter()
                .flat_map(|kind| indexed_incoming_edges_by_kind(graph, node_id, *kind)),
        );
        if include_requirements {
            edges.extend(
                requirement_edge_kinds()
                    .iter()
                    .flat_map(|kind| indexed_incoming_edges_by_kind(graph, node_id, *kind)),
            );
        }
    }
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    edges.dedup_by(|left, right| left.edge_id == right.edge_id);
    edges
}

pub fn sdg_edge_kinds() -> &'static [EdgeKind] {
    &[
        EdgeKind::Controls,
        EdgeKind::DataFlow,
        EdgeKind::Calls,
        EdgeKind::ParameterIn,
        EdgeKind::ReturnsTo,
        EdgeKind::ParameterOut,
        EdgeKind::ThrowsTo,
    ]
}

pub fn is_locatable_code_fact(node: &GraphNode) -> bool {
    !matches!(
        node.kind,
        sg::NodeKind::Requirement | sg::NodeKind::DomainKnowledge | sg::NodeKind::Diagnostic
    ) && (node.span.is_some()
        || matches!(
            node.kind,
            sg::NodeKind::Artifact | sg::NodeKind::Callable | sg::NodeKind::Scope
        ))
}

pub fn seed_values_for_behavior(graph: &ProgramSupergraph, behavior_id: NodeId) -> BTreeSet<NodeId> {
    let mut value_ids = BTreeSet::new();
    let Some(node) = indexed_node(graph, behavior_id) else {
        return value_ids;
    };

    match &node.fact {
        NodeFact::Value(value) => {
            value_ids.insert(value.value_id);
        }
        NodeFact::Definition(definition) => {
            if let Some(value_id) = &definition.value_id {
                value_ids.insert(*value_id);
            }
        }
        NodeFact::Use(use_fact) => {
            if let Some(value_id) = &use_fact.value_id {
                value_ids.insert(*value_id);
            }
        }
        NodeFact::Expression(expression) => {
            if let Some(value_id) = &expression.value_id {
                value_ids.insert(*value_id);
            }
            value_ids.extend(child_expression_values(graph, expression.expression_id));
        }
        NodeFact::Statement(statement) => {
            for expression_id in &statement.expression_ids {
                value_ids.extend(expression_tree_values(graph, *expression_id));
            }
        }
        NodeFact::Condition(condition) => {
            if let Some(expression_id) = &condition.expression_id {
                value_ids.extend(expression_tree_values(graph, *expression_id));
            }
        }
        NodeFact::ControlFlow(control) => match control.role {
            ControlFlowNodeRole::Return
            | ControlFlowNodeRole::Raise
            | ControlFlowNodeRole::Condition
            | ControlFlowNodeRole::Statement => {
                if let Some(span) = node.span {
                    value_ids.extend(values_at_span(graph, span, control.callable_id));
                }
            }
            ControlFlowNodeRole::Entry | ControlFlowNodeRole::Exit | ControlFlowNodeRole::Merge => {
            }
        },
        NodeFact::DataFlow(data_flow) => {
            if data_flow.role == DataFlowNodeRole::Definition
                && let Some(span) = node.span
            {
                value_ids.extend(values_at_span(graph, span, data_flow.callable_id));
            }
        }
        NodeFact::CallSite(call_site) => {
            if let Some(span) = node.span {
                value_ids.extend(values_at_span(
                    graph,
                    span,
                    call_site.enclosing_callable_id,
                ));
            }
        }
        NodeFact::Artifact(_)
        | NodeFact::Scope(_)
        | NodeFact::Binding(_)
        | NodeFact::Callable(_)
        | NodeFact::ExternalTarget(_)
        | NodeFact::Symbol(_)
        | NodeFact::BasicBlock(_)
        | NodeFact::DomainKnowledge(_)
        | NodeFact::Requirement(_)
        | NodeFact::Diagnostic(_) => {}
    }

    value_ids
}

pub fn child_expression_values(graph: &ProgramSupergraph, expression_id: NodeId) -> BTreeSet<NodeId> {
    let Some(node) = indexed_node(graph, expression_id) else {
        return BTreeSet::new();
    };
    let NodeFact::Expression(expression) = &node.fact else {
        return BTreeSet::new();
    };

    expression
        .child_expression_ids
        .iter()
        .flat_map(|child_id| expression_tree_values(graph, *child_id))
        .collect()
}

pub fn expression_tree_values(graph: &ProgramSupergraph, expression_id: NodeId) -> BTreeSet<NodeId> {
    let mut value_ids = BTreeSet::new();
    let Some(node) = indexed_node(graph, expression_id) else {
        return value_ids;
    };
    let NodeFact::Expression(expression) = &node.fact else {
        return value_ids;
    };

    if let Some(value_id) = &expression.value_id {
        value_ids.insert(*value_id);
    }
    for child_id in &expression.child_expression_ids {
        value_ids.extend(expression_tree_values(graph, *child_id));
    }
    value_ids
}

pub fn values_at_span(
    graph: &ProgramSupergraph,
    span: SourceSpan,
    callable_id: NodeId,
) -> BTreeSet<NodeId> {
    graph
        .nodes
        .iter()
        .filter(|node| {
            node.span
                .is_some_and(|node_span| spans_overlap(node_span, span))
        })
        .filter_map(|node| match &node.fact {
            NodeFact::Value(value) if value.callable_id == Some(callable_id) => {
                Some(value.value_id)
            }
            NodeFact::Expression(expression) if expression.callable_id == callable_id => {
                expression.value_id
            }
            NodeFact::Definition(definition) if definition.callable_id == callable_id => {
                definition.value_id
            }
            NodeFact::Use(use_fact) if use_fact.callable_id == callable_id => {
                use_fact.value_id
            }
            _ => None,
        })
        .collect()
}

pub fn spans_overlap(left: SourceSpan, right: SourceSpan) -> bool {
    left.start_byte < right.end_byte && right.start_byte < left.end_byte
}

pub fn incoming_control_edges<'a>(
    graph: &'a ProgramSupergraph,
    behavior_id: NodeId,
) -> Vec<&'a GraphEdge> {
    let mut controlled_ids = BTreeSet::from([behavior_id]);
    controlled_ids.extend(related_cfg_ids_for_behavior(graph, behavior_id));
    controlled_ids.extend(related_behavior_projection_ids(graph, behavior_id));

    controlled_ids
        .iter()
        .flat_map(|controlled_id| {
            indexed_incoming_edges_by_kind(graph, *controlled_id, EdgeKind::Controls)
        })
        .filter(|edge| matches!(&edge.fact, EdgeFact::Controls(_)))
        .collect()
}

pub fn related_behavior_projection_ids(
    graph: &ProgramSupergraph,
    behavior_id: NodeId,
) -> BTreeSet<NodeId> {
    let mut related_ids = BTreeSet::new();
    let Some(node) = indexed_node(graph, behavior_id) else {
        return related_ids;
    };
    let Some(span) = node.span else {
        return related_ids;
    };
    let callable_id = node
        .owner
        .callable_id
        .or_else(|| match &node.fact {
            NodeFact::CallSite(call_site) => Some(call_site.enclosing_callable_id),
            NodeFact::Statement(statement) => Some(statement.callable_id),
            NodeFact::Expression(expression) => Some(expression.callable_id),
            NodeFact::Definition(definition) => Some(definition.callable_id),
            NodeFact::Use(use_fact) => Some(use_fact.callable_id),
            NodeFact::Value(value) => value.callable_id,
            NodeFact::ControlFlow(control) => Some(control.callable_id),
            NodeFact::DataFlow(data_flow) => Some(data_flow.callable_id),
            _ => None,
        });
    let Some(callable_id) = callable_id else {
        return related_ids;
    };

    for candidate in graph.nodes.iter().filter(|candidate| {
        candidate
            .span
            .is_some_and(|candidate_span| spans_overlap(candidate_span, span))
    }) {
        match &candidate.fact {
            NodeFact::Statement(statement) if statement.callable_id == callable_id => {
                related_ids.insert(candidate.node_id);
            }
            NodeFact::Expression(expression) if expression.callable_id == callable_id => {
                related_ids.insert(candidate.node_id);
            }
            NodeFact::DataFlow(data_flow) if data_flow.callable_id == callable_id => {
                related_ids.insert(candidate.node_id);
            }
            NodeFact::CallSite(call_site) if call_site.enclosing_callable_id == callable_id => {
                related_ids.insert(candidate.node_id);
            }
            _ => {}
        }
    }

    related_ids
}

pub fn related_cfg_ids_for_behavior(graph: &ProgramSupergraph, behavior_id: NodeId) -> BTreeSet<NodeId> {
    let mut cfg_ids = BTreeSet::new();
    let Some(node) = indexed_node(graph, behavior_id) else {
        return cfg_ids;
    };
    let Some(span) = node.span else {
        return cfg_ids;
    };
    let callable_id = node
        .owner
        .callable_id
        .or_else(|| match &node.fact {
            NodeFact::CallSite(call_site) => Some(call_site.enclosing_callable_id),
            NodeFact::Statement(statement) => Some(statement.callable_id),
            NodeFact::Expression(expression) => Some(expression.callable_id),
            NodeFact::Definition(definition) => Some(definition.callable_id),
            NodeFact::Use(use_fact) => Some(use_fact.callable_id),
            NodeFact::Value(value) => value.callable_id,
            NodeFact::ControlFlow(control) => Some(control.callable_id),
            NodeFact::DataFlow(data_flow) => Some(data_flow.callable_id),
            _ => None,
        });
    let Some(callable_id) = callable_id else {
        return cfg_ids;
    };

    for candidate in indexed_nodes_by_kind(graph, sg::NodeKind::ControlFlow) {
        if !candidate
            .span
            .is_some_and(|candidate_span| spans_overlap(candidate_span, span))
        {
            continue;
        }
        if let NodeFact::ControlFlow(control) = &candidate.fact
            && control.callable_id == callable_id
        {
            cfg_ids.insert(candidate.node_id);
        }
    }

    cfg_ids
}

pub fn path_conditions_for_behavior(
    graph: &ProgramSupergraph,
    behavior_id: NodeId,
    controls: &[&GraphEdge],
) -> Vec<sg::PathConditionSummary> {
    let mut summaries = Vec::new();

    for requirement_id in graph
        .indexes
        .code_to_requirements
        .get(&behavior_id)
        .into_iter()
        .flatten()
    {
        if let Some(requirement) =
            indexed_node(graph, *requirement_id).and_then(|node| match &node.fact {
                NodeFact::Requirement(requirement) => Some(requirement),
                _ => None,
            })
        {
            summaries.extend(requirement.path_conditions.iter().cloned());
        }
    }

    for control_edge in controls {
        let EdgeFact::Controls(controls) = &control_edge.fact else {
            continue;
        };
        summaries.push(sg::PathConditionSummary {
            controlling_cfg_node_id: controls.condition_id,
            condition_id: structured_condition_id_for_cfg_node(graph, controls.condition_id),
            expression_id: structured_condition_id_for_cfg_node(graph, controls.condition_id)
                .and_then(|condition_id| indexed_node(graph, condition_id))
                .and_then(|node| match &node.fact {
                    NodeFact::Condition(condition) => condition.expression_id,
                    _ => None,
                }),
            outcome: sg::ControlFlowOutcome::Unknown,
            branch_arm: None,
            summary: Sym::from(condition_summary(graph, controls.condition_id)),
        });
    }

    summaries.sort_by(|left, right| {
        (
            left.condition_id,
            left.controlling_cfg_node_id,
            left.outcome,
            left.summary.as_str(),
        )
            .cmp(&(
                right.condition_id,
                right.controlling_cfg_node_id,
                right.outcome,
                right.summary.as_str(),
            ))
    });
    summaries.dedup();
    summaries
}

pub fn structured_condition_id_for_cfg_node(
    graph: &ProgramSupergraph,
    cfg_node_id: NodeId,
) -> Option<NodeId> {
    let cfg_node = indexed_node(graph, cfg_node_id)?;
    let span = cfg_node.span?;
    let NodeFact::ControlFlow(control) = &cfg_node.fact else {
        return None;
    };

    indexed_nodes_by_kind(graph, sg::NodeKind::Condition)
        .into_iter()
        .find_map(|node| match &node.fact {
            NodeFact::Condition(condition)
                if condition.callable_id == control.callable_id && node.span == Some(span) =>
            {
                Some(condition.condition_id)
            }
            _ => None,
        })
}

pub fn condition_summary(graph: &ProgramSupergraph, cfg_node_id: NodeId) -> String {
    indexed_node(graph, cfg_node_id)
        .and_then(|node| match &node.fact {
            NodeFact::ControlFlow(control) => Some(format!("controlled by `{}`", control.label)),
            _ => None,
        })
        .unwrap_or_else(|| format!("controlled by {cfg_node_id}"))
}

pub fn diagnostic_ids_for_slice(
    graph: &ProgramSupergraph,
    value_ids: &BTreeSet<NodeId>,
    edge_ids: &BTreeSet<sg::EdgeId>,
) -> Vec<NodeId> {
    let mut related_ids = value_ids.clone();
    for edge_id in edge_ids {
        related_ids.insert(NodeId::from(*edge_id));
        if let Some(edge) = indexed_edge(graph, *edge_id) {
            related_ids.insert(edge.source_id);
            if let Some(target_id) = &edge.target_id {
                related_ids.insert(*target_id);
            }
        }
    }
    for value_id in value_ids {
        if let Some(NodeFact::Value(value)) = indexed_node(graph, *value_id).map(|node| &node.fact) {
            related_ids.extend(value.expression_id.iter().copied());
            related_ids.extend(value.call_site_id.iter().copied());
            related_ids.extend(value.symbol_id.iter().copied());
            related_ids.extend(value.state_of_value_id.iter().copied());
        }
    }

    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Diagnostic(diagnostic)
                if diagnostic
                    .related
                    .iter()
                    .any(|related_id| related_ids.contains(related_id)) =>
            {
                Some(diagnostic.diagnostic_id)
            }
            _ => None,
        })
        .collect()
}

pub fn is_value_node(graph: &ProgramSupergraph, node_id: NodeId) -> bool {
    indexed_node(graph, node_id).is_some_and(|node| matches!(&node.fact, NodeFact::Value(_)))
}

pub fn dfg_edge_kinds() -> &'static [EdgeKind] {
    &[
        EdgeKind::Defines,
        EdgeKind::Uses,
        EdgeKind::DataFlow,
        EdgeKind::ParameterIn,
        EdgeKind::ReturnsTo,
        EdgeKind::ParameterOut,
        EdgeKind::ThrowsTo,
    ]
}

pub fn dfg_value_edge_kinds() -> &'static [EdgeKind] {
    &[
        EdgeKind::DataFlow,
        EdgeKind::ParameterIn,
        EdgeKind::ReturnsTo,
        EdgeKind::ParameterOut,
        EdgeKind::ThrowsTo,
    ]
}

pub fn edge_mentions_callable(edge: &GraphEdge, callable_id: NodeId) -> bool {
    match &edge.fact {
        EdgeFact::Defines(defines) => defines.callable_id == callable_id,
        EdgeFact::Uses(uses) => uses.callable_id == callable_id,
        EdgeFact::DataFlow(data_flow) => data_flow.callable_id == callable_id,
        EdgeFact::ParameterIn(parameter) => {
            parameter.caller_callable_id == callable_id
                || parameter.callee_callable_id == callable_id
        }
        EdgeFact::ReturnsTo(returns) => {
            returns.caller_callable_id == callable_id || returns.callee_callable_id == callable_id
        }
        EdgeFact::ParameterOut(parameter) => {
            parameter.caller_callable_id == callable_id
                || parameter.callee_callable_id == callable_id
        }
        EdgeFact::ThrowsTo(throws) => {
            throws.caller_callable_id == callable_id || throws.callee_callable_id == callable_id
        }
        _ => false,
    }
}

pub fn indexed_outgoing_edges_by_kind<'a>(
    graph: &'a ProgramSupergraph,
    node_id: NodeId,
    kind: EdgeKind,
) -> Vec<&'a GraphEdge> {
    graph
        .indexes
        .outgoing_edges_by_node
        .get(&node_id)
        .into_iter()
        .flatten()
        .filter_map(|edge_id| indexed_edge(graph, *edge_id))
        .filter(|edge| edge.kind == kind)
        .collect()
}

pub fn indexed_incoming_edges_by_kind<'a>(
    graph: &'a ProgramSupergraph,
    node_id: NodeId,
    kind: EdgeKind,
) -> Vec<&'a GraphEdge> {
    graph
        .indexes
        .incoming_edges_by_node
        .get(&node_id)
        .into_iter()
        .flatten()
        .filter_map(|edge_id| indexed_edge(graph, *edge_id))
        .filter(|edge| edge.kind == kind)
        .collect()
}

pub fn indexed_node<'a>(graph: &'a ProgramSupergraph, node_id: NodeId) -> Option<&'a GraphNode> {
    graph
        .indexes
        .node_position_by_id
        .get(&node_id)
        .and_then(|position| graph.nodes.get(*position as usize))
        .filter(|node| node.node_id == node_id)
}

pub fn indexed_edge<'a>(graph: &'a ProgramSupergraph, edge_id: sg::EdgeId) -> Option<&'a GraphEdge> {
    graph
        .indexes
        .edge_position_by_id
        .get(&edge_id)
        .and_then(|position| graph.edges.get(*position as usize))
        .filter(|edge| edge.edge_id == edge_id)
}

pub fn indexed_nodes_by_kind(graph: &ProgramSupergraph, kind: sg::NodeKind) -> Vec<&GraphNode> {
    graph
        .indexes
        .nodes_by_kind
        .get(&kind)
        .into_iter()
        .flatten()
        .filter_map(|node_id| indexed_node(graph, *node_id))
        .collect()
}

pub fn indexed_edges_by_kinds<'a>(
    graph: &'a ProgramSupergraph,
    kinds: &[EdgeKind],
) -> Vec<&'a GraphEdge> {
    kinds
        .iter()
        .filter_map(|kind| graph.indexes.edges_by_kind.get(kind))
        .flatten()
        .filter_map(|edge_id| indexed_edge(graph, *edge_id))
        .collect()
}

pub fn indexed_call_edge_ids(graph: &ProgramSupergraph, filter: &CallGraphFilter) -> Vec<sg::EdgeId> {
    let mut edge_ids = BTreeSet::new();
    if !filter.caller_ids.is_empty() {
        for caller_id in &filter.caller_ids {
            if let Some(calls) = graph.indexes.calls_by_caller.get(caller_id) {
                edge_ids.extend(calls.iter().copied());
            }
        }
    } else if !filter.callee_ids.is_empty() {
        for target_id in &filter.callee_ids {
            if let Some(calls) = graph.indexes.calls_by_concrete_target.get(target_id) {
                edge_ids.extend(calls.iter().copied());
            }
        }
    } else if !filter.call_site_ids.is_empty() {
        for call_site_id in &filter.call_site_ids {
            if let Some(calls) = graph.indexes.call_site_to_calls.get(call_site_id) {
                edge_ids.extend(calls.iter().copied());
            }
        }
    } else if !filter.artifact_ids.is_empty() {
        for artifact_id in &filter.artifact_ids {
            if let Some(edges) = graph.indexes.owner_to_edges.get(artifact_id) {
                edge_ids.extend(edges.iter().copied());
            }
        }
    } else if let Some(calls) = graph.indexes.edges_by_kind.get(&EdgeKind::Calls) {
        edge_ids.extend(calls.iter().copied());
    }

    edge_ids.into_iter().collect()
}


pub fn requirement_edge_kinds() -> &'static [EdgeKind] {
    &[
        EdgeKind::TracesTo,
        EdgeKind::DecomposesTo,
        EdgeKind::Conditions,
        EdgeKind::Orders,
    ]
}

pub fn related_code_ids_for_value(graph: &ProgramSupergraph, value_id: NodeId) -> BTreeSet<NodeId> {
    let mut related_ids = BTreeSet::new();
    let Some(NodeFact::Value(value)) = indexed_node(graph, value_id).map(|node| &node.fact) else {
        return related_ids;
    };

    related_ids.extend(value.expression_id.iter().copied());
    related_ids.extend(value.call_site_id.iter().copied());
    for node in &graph.nodes {
        match &node.fact {
            NodeFact::Definition(definition)
                if definition.value_id == Some(value_id) =>
            {
                related_ids.insert(definition.definition_id);
            }
            NodeFact::Use(use_fact) if use_fact.value_id == Some(value_id) => {
                related_ids.insert(use_fact.use_id);
            }
            _ => {}
        }
    }

    related_ids
}

pub fn requirements_for_code_or_related_value(
    graph: &ProgramSupergraph,
    code_or_value_id: NodeId,
) -> BTreeSet<NodeId> {
    let mut requirement_ids = graph
        .indexes
        .code_to_requirements
        .get(&code_or_value_id)
        .into_iter()
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>();
    for related_id in related_code_ids_for_value(graph, code_or_value_id) {
        requirement_ids.extend(
            graph
                .indexes
                .code_to_requirements
                .get(&related_id)
                .into_iter()
                .flatten()
                .copied(),
        );
    }
    requirement_ids
}

pub fn callable_ids_for_slice(
    graph: &ProgramSupergraph,
    node_ids: &BTreeSet<NodeId>,
    edge_ids: &BTreeSet<sg::EdgeId>,
) -> Vec<NodeId> {
    let mut callable_ids = BTreeSet::new();
    for node_id in node_ids {
        if let Some(node) = indexed_node(graph, *node_id) {
            callable_ids.extend(node.owner.callable_id.iter().copied());
            match &node.fact {
                NodeFact::Callable(callable) => {
                    callable_ids.insert(callable.callable_id);
                }
                NodeFact::CallSite(call_site) => {
                    callable_ids.insert(call_site.enclosing_callable_id);
                }
                NodeFact::Value(value) => {
                    callable_ids.extend(value.callable_id.iter().copied());
                }
                NodeFact::ControlFlow(control) => {
                    callable_ids.insert(control.callable_id);
                }
                NodeFact::DataFlow(data_flow) => {
                    callable_ids.insert(data_flow.callable_id);
                }
                NodeFact::Definition(definition) => {
                    callable_ids.insert(definition.callable_id);
                }
                NodeFact::Use(use_fact) => {
                    callable_ids.insert(use_fact.callable_id);
                }
                NodeFact::Expression(expression) => {
                    callable_ids.insert(expression.callable_id);
                }
                NodeFact::Statement(statement) => {
                    callable_ids.insert(statement.callable_id);
                }
                NodeFact::Condition(condition) => {
                    callable_ids.insert(condition.callable_id);
                }
                _ => {}
            }
        }
    }
    for edge_id in edge_ids {
        if let Some(edge) = indexed_edge(graph, *edge_id) {
            match &edge.fact {
                EdgeFact::Controls(controls) => {
                    callable_ids.insert(controls.callable_id);
                }
                EdgeFact::DataFlow(data_flow) => {
                    callable_ids.insert(data_flow.callable_id);
                }
                EdgeFact::Calls(calls) => {
                    callable_ids.insert(calls.caller_callable_id);
                    callable_ids.extend(calls.callee_callable_id.iter().copied());
                }
                EdgeFact::ParameterIn(parameter) => {
                    callable_ids.insert(parameter.caller_callable_id);
                    callable_ids.insert(parameter.callee_callable_id);
                }
                EdgeFact::ReturnsTo(returns) => {
                    callable_ids.insert(returns.caller_callable_id);
                    callable_ids.insert(returns.callee_callable_id);
                }
                EdgeFact::ParameterOut(parameter) => {
                    callable_ids.insert(parameter.caller_callable_id);
                    callable_ids.insert(parameter.callee_callable_id);
                }
                EdgeFact::ThrowsTo(throws) => {
                    callable_ids.insert(throws.caller_callable_id);
                    callable_ids.insert(throws.callee_callable_id);
                }
                _ => {}
            }
        }
    }
    callable_ids.into_iter().collect()
}

pub fn caller_and_call_site_for_impact(edge: &GraphEdge) -> Option<(NodeId, NodeId)> {
    match &edge.fact {
        EdgeFact::Calls(calls) => {
            Some((calls.caller_callable_id, calls.call_site_id))
        }
        EdgeFact::ParameterIn(parameter) => Some((
            parameter.caller_callable_id,
            parameter.call_site_id,
        )),
        EdgeFact::ReturnsTo(returns) => Some((
            returns.caller_callable_id,
            returns.call_site_id,
        )),
        EdgeFact::ParameterOut(parameter) => Some((
            parameter.caller_callable_id,
            parameter.call_site_id,
        )),
        EdgeFact::ThrowsTo(throws) => Some((
            throws.caller_callable_id,
            throws.call_site_id,
        )),
        _ => None,
    }
}

pub fn requirements_for_edges(
    graph: &ProgramSupergraph,
    edge_ids: &BTreeSet<sg::EdgeId>,
) -> Vec<NodeId> {
    let mut requirement_ids = BTreeSet::new();
    for edge_id in edge_ids {
        requirement_ids.extend(
            graph
                .indexes
                .code_to_requirements
                .get(&NodeId::from(*edge_id))
                .into_iter()
                .flatten()
                .copied(),
        );
        if let Some(edge) = indexed_edge(graph, *edge_id) {
            requirement_ids.extend(
                graph
                    .indexes
                    .code_to_requirements
                    .get(&edge.source_id)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
            if let Some(target_id) = &edge.target_id {
                requirement_ids.extend(
                    graph
                        .indexes
                        .code_to_requirements
                        .get(target_id)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
        }
    }
    requirement_ids.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supergraph::builder::ProgramSupergraphBuilder;
    use crate::supergraph::ids::test_support::{test_edge_id, test_id};
    use crate::supergraph::schema::{
        Confidence, Diagnostic, DiagnosticKind, Evidence, EvidenceKind, Severity,
    };

    fn graph_with_diagnostic_related_to(related: NodeId, diagnostic_id: NodeId) -> ProgramSupergraph {
        let mut builder = ProgramSupergraphBuilder::new("repo", "rust");
        builder.add_diagnostic(
            Diagnostic {
                diagnostic_id,
                kind: DiagnosticKind::AliasUncertainty,
                severity: Severity::Warning,
                message: "m".to_string(),
                artifact_id: None,
                span: None,
                related: vec![related],
            },
            Confidence::Unknown,
            vec![Evidence {
                kind: EvidenceKind::Inference,
                summary: Sym::from("s".to_string()),
                source_id: None,
                source_span: None,
                content_hash: None,
                syntax: None,
            }],
        );
        builder.finish()
    }

    #[test]
    fn diagnostic_slice_includes_diagnostics_related_to_slice_edges() {
        let edge_id = test_edge_id("slice-edge");
        let diagnostic_id = test_id("diagnostic:edge-related");
        let graph = graph_with_diagnostic_related_to(NodeId::from(edge_id), diagnostic_id);

        let found = diagnostic_ids_for_slice(&graph, &BTreeSet::new(), &BTreeSet::from([edge_id]));
        assert_eq!(found, vec![diagnostic_id]);

        let other = BTreeSet::from([test_edge_id("other-edge")]);
        assert!(diagnostic_ids_for_slice(&graph, &BTreeSet::new(), &other).is_empty());
    }

    #[test]
    fn requirements_for_edges_finds_requirements_traced_to_the_edge_itself() {
        let edge_id = test_edge_id("traced-edge");
        let requirement_id = test_id("requirement:edge-trace");
        let mut builder = ProgramSupergraphBuilder::new("repo", "rust");
        builder.add_traces_to(
            sg::TracesTo {
                requirement_id,
                code_fact_id: NodeId::from(edge_id),
                precision: Sym::new("edge-trace"),
            },
            sg::SourceOwnership::default(),
            None,
            Confidence::Exact,
            vec![Evidence {
                kind: EvidenceKind::Inference,
                summary: Sym::from("s".to_string()),
                source_id: None,
                source_span: None,
                content_hash: None,
                syntax: None,
            }],
        );
        let graph = builder.finish();

        assert_eq!(
            requirements_for_edges(&graph, &BTreeSet::from([edge_id])),
            vec![requirement_id]
        );
        let other = BTreeSet::from([test_edge_id("other-edge")]);
        assert!(requirements_for_edges(&graph, &other).is_empty());
    }
}
