use std::collections::HashMap;

use crate::{
    ast::SourceSpan,
    supergraph::{
        BindingKind, EdgeFact, GraphEdge, GraphNode, NodeFact, NodeId, ProgramSupergraph,
        ids::{IdMap, IdSet},
    },
};

/// Snapshot index over a graph that replaces whole-graph scans with per-callable lookups.
///
/// Entries are positions into `graph.nodes`/`graph.edges`, so the index stays valid while
/// the graph is only appended to. Nodes added after the snapshot are not visible; passes
/// that need those must not rely on this index for them.
#[derive(Debug, Default)]
pub(crate) struct CallableIndex {
    nodes_seen: usize,
    position_by_id: IdMap<NodeId, usize>,
    edges_seen: usize,
    expression_positions: IdMap<NodeId, usize>,
    parameter_binding_spans: HashMap<(NodeId, String), SourceSpan>,
    nodes_by_callable: IdMap<NodeId, Vec<usize>>,
    control_flow_edges_by_callable: IdMap<NodeId, Vec<usize>>,
    node_position_by_cfg_id: IdMap<NodeId, usize>,
    expression_spans: IdMap<NodeId, SourceSpan>,
    definition_values: HashMap<(NodeId, String, SourceSpan), NodeId>,
    use_values: HashMap<(NodeId, String, SourceSpan), NodeId>,
    value_ids: IdSet<NodeId>,
}

impl CallableIndex {
    pub(crate) fn build(graph: &ProgramSupergraph) -> Self {
        let mut index = Self::default();
        index.sync(graph);
        index
    }

    /// Indexes nodes and edges appended to `graph` since the last `build`/`sync`.
    pub(crate) fn sync(&mut self, graph: &ProgramSupergraph) {
        for position in self.nodes_seen..graph.nodes.len() {
            self.index_node(position, &graph.nodes[position]);
        }
        self.nodes_seen = graph.nodes.len();
        for position in self.edges_seen..graph.edges.len() {
            if let EdgeFact::ControlFlow(flow) = &graph.edges[position].fact {
                self.control_flow_edges_by_callable
                    .entry(flow.callable_id.clone())
                    .or_default()
                    .push(position);
            }
        }
        self.edges_seen = graph.edges.len();
    }

    fn index_node(&mut self, position: usize, node: &GraphNode) {
        self.position_by_id
            .entry(node.node_id.clone())
            .or_insert(position);
        if let Some(callable_id) = callable_id_of(&node.fact) {
            self.nodes_by_callable
                .entry(callable_id)
                .or_default()
                .push(position);
        }
        match &node.fact {
            NodeFact::ControlFlow(control) => {
                self.node_position_by_cfg_id
                    .entry(control.cfg_node_id.clone())
                    .or_insert(position);
            }
            NodeFact::Expression(expression) => {
                self.expression_positions
                    .entry(expression.expression_id.clone())
                    .or_insert(position);
                if let Some(span) = node.span {
                    self.expression_spans
                        .entry(expression.expression_id.clone())
                        .or_insert(span);
                }
            }
            NodeFact::Binding(binding) if binding.kind == BindingKind::Parameter => {
                self.parameter_binding_spans
                    .entry((binding.scope_id.clone(), binding.name.clone()))
                    .or_insert(binding.span);
            }
            NodeFact::Definition(definition) => {
                if let (Some(name), Some(span), Some(value_id)) =
                    (&definition.name, node.span, &definition.value_id)
                {
                    self.definition_values
                        .entry((definition.callable_id.clone(), name.clone(), span))
                        .or_insert_with(|| value_id.clone());
                }
            }
            NodeFact::Use(use_fact) => {
                if let (Some(name), Some(span), Some(value_id)) =
                    (&use_fact.name, node.span, &use_fact.value_id)
                {
                    self.use_values
                        .entry((use_fact.callable_id.clone(), name.clone(), span))
                        .or_insert_with(|| value_id.clone());
                }
            }
            NodeFact::Value(value) => {
                self.value_ids.insert(value.value_id.clone());
            }
            _ => {}
        }
    }

    /// Positions in `graph.nodes` of the nodes owned by `callable_id`, in graph order.
    pub(crate) fn positions(&self, callable_id: NodeId) -> &[usize] {
        self.nodes_by_callable
            .get(&callable_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Position in `graph.nodes` of the first node with `node_id`.
    pub(crate) fn node_position(&self, node_id: NodeId) -> Option<usize> {
        self.position_by_id.get(&node_id).copied()
    }

    pub(crate) fn expression_position(&self, expression_id: NodeId) -> Option<usize> {
        self.expression_positions.get(&expression_id).copied()
    }

    pub(crate) fn parameter_binding_span(
        &self,
        scope_id: NodeId,
        parameter_name: &str,
    ) -> Option<SourceSpan> {
        self.parameter_binding_spans
            .get(&(scope_id, parameter_name.to_string()))
            .copied()
    }

    /// Nodes owned by `callable_id`, in graph order.
    pub(crate) fn nodes<'g>(
        &self,
        graph: &'g ProgramSupergraph,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'g GraphNode> {
        self.nodes_by_callable
            .get(&callable_id)
            .into_iter()
            .flatten()
            .map(|position| &graph.nodes[*position])
    }

    /// Control-flow edges of `callable_id`, in graph order.
    pub(crate) fn control_flow_edges<'g>(
        &self,
        graph: &'g ProgramSupergraph,
        callable_id: NodeId,
    ) -> impl Iterator<Item = &'g GraphEdge> {
        self.control_flow_edges_by_callable
            .get(&callable_id)
            .into_iter()
            .flatten()
            .map(|position| &graph.edges[*position])
    }

    pub(crate) fn control_flow_node<'g>(
        &self,
        graph: &'g ProgramSupergraph,
        cfg_node_id: NodeId,
    ) -> Option<&'g GraphNode> {
        self.node_position_by_cfg_id
            .get(&cfg_node_id)
            .map(|position| &graph.nodes[*position])
    }

    pub(crate) fn expression_span(&self, expression_id: NodeId) -> Option<SourceSpan> {
        self.expression_spans.get(&expression_id).copied()
    }

    pub(crate) fn definition_value_id(
        &self,
        callable_id: NodeId,
        name: &str,
        span: SourceSpan,
    ) -> Option<NodeId> {
        self.definition_values
            .get(&(callable_id, name.to_string(), span))
            .cloned()
    }

    pub(crate) fn use_value_id(
        &self,
        callable_id: NodeId,
        name: &str,
        span: SourceSpan,
    ) -> Option<NodeId> {
        self.use_values
            .get(&(callable_id, name.to_string(), span))
            .cloned()
    }

    pub(crate) fn has_value(&self, value_id: NodeId) -> bool {
        self.value_ids.contains(&value_id)
    }
}

fn callable_id_of(fact: &NodeFact) -> Option<NodeId> {
    match fact {
        NodeFact::CallSite(fact) => Some(fact.enclosing_callable_id),
        NodeFact::Statement(fact) => Some(fact.callable_id),
        NodeFact::Expression(fact) => Some(fact.callable_id),
        NodeFact::Condition(fact) => Some(fact.callable_id),
        NodeFact::Definition(fact) => Some(fact.callable_id),
        NodeFact::Use(fact) => Some(fact.callable_id),
        NodeFact::Value(fact) => fact.callable_id,
        NodeFact::ControlFlow(fact) => Some(fact.callable_id),
        NodeFact::DataFlow(fact) => Some(fact.callable_id),
        _ => None,
    }
}
