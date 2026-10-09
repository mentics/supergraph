use std::cmp::Ordering;
use std::collections::{BTreeSet, VecDeque};
use std::fmt;

use super::ids::EdgeId;
use super::{
    EdgeFact, EdgeKind, GraphEdge, GraphNode, NodeFact, NodeId, NodeKind, ProgramSupergraph,
    SourceSpanIndexKey,
};

/// A node or an edge that can be invalidated. Ordered like the `prefix:hex16` text of the
/// underlying id (so nodes and edges interleave by prefix, as the old string ids did).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidationSubject {
    Node(NodeId),
    Edge(EdgeId),
}

impl InvalidationSubject {
    fn sort_key(&self) -> (&'static str, u64) {
        match self {
            InvalidationSubject::Node(id) => (id.tag.prefix(), id.hash.get()),
            InvalidationSubject::Edge(id) => (EdgeId::PREFIX, id.hash().get()),
        }
    }

    pub fn as_node(&self) -> Option<NodeId> {
        match self {
            InvalidationSubject::Node(id) => Some(*id),
            InvalidationSubject::Edge(_) => None,
        }
    }

    pub fn as_edge(&self) -> Option<EdgeId> {
        match self {
            InvalidationSubject::Edge(id) => Some(*id),
            InvalidationSubject::Node(_) => None,
        }
    }
}

impl Ord for InvalidationSubject {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for InvalidationSubject {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<NodeId> for InvalidationSubject {
    fn from(id: NodeId) -> Self {
        InvalidationSubject::Node(id)
    }
}

impl From<EdgeId> for InvalidationSubject {
    fn from(id: EdgeId) -> Self {
        InvalidationSubject::Edge(id)
    }
}

impl From<&NodeId> for InvalidationSubject {
    fn from(id: &NodeId) -> Self {
        InvalidationSubject::Node(*id)
    }
}

impl From<&EdgeId> for InvalidationSubject {
    fn from(id: &EdgeId) -> Self {
        InvalidationSubject::Edge(*id)
    }
}

impl fmt::Display for InvalidationSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidationSubject::Node(id) => id.fmt(f),
            InvalidationSubject::Edge(id) => id.fmt(f),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InvalidationDependency {
    pub invalidator_id: InvalidationSubject,
    pub derived_id: InvalidationSubject,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArtifactHashChange {
    pub artifact_id: NodeId,
    pub new_content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceChangeInvalidation {
    pub changed_artifact_ids: BTreeSet<NodeId>,
    pub direct_dirty_subject_ids: BTreeSet<InvalidationSubject>,
    pub dirty_subject_ids: BTreeSet<InvalidationSubject>,
    pub dirty_generated_view_subject_ids: BTreeSet<InvalidationSubject>,
}

pub fn invalidated_by(
    graph: &ProgramSupergraph,
    invalidator_id: impl Into<InvalidationSubject>,
) -> Vec<InvalidationDependency> {
    invalidated_by_with_options(graph, invalidator_id.into(), true, true)
}

fn invalidated_by_with_options(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    allow_requirement_to_code: bool,
    allow_requirement_expansion: bool,
) -> Vec<InvalidationDependency> {
    let mut dependencies = BTreeSet::new();
    if !allow_requirement_expansion
        && subject_node(graph, invalidator_id).is_some_and(|node| node.kind == NodeKind::Requirement)
    {
        return Vec::new();
    }

    add_owned_subjects(graph, invalidator_id, &mut dependencies);
    add_same_span_subjects(graph, invalidator_id, &mut dependencies);
    add_endpoint_edges(graph, invalidator_id, &mut dependencies);
    add_trace_subjects(
        graph,
        invalidator_id,
        allow_requirement_to_code,
        &mut dependencies,
    );
    add_domain_subjects(graph, invalidator_id, &mut dependencies);
    add_edge_derived_subjects(graph, invalidator_id, &mut dependencies);

    dependencies.into_iter().collect()
}

pub fn invalidation_closure(
    graph: &ProgramSupergraph,
    invalidator_id: impl Into<InvalidationSubject>,
) -> BTreeSet<InvalidationSubject> {
    let invalidator_id = invalidator_id.into();
    let mut dirty = BTreeSet::new();
    let mut frontier = VecDeque::from([invalidator_id]);

    while let Some(current_id) = frontier.pop_front() {
        let allow_requirement_to_code = current_id == invalidator_id;
        let allow_requirement_expansion = current_id == invalidator_id;
        for dependency in invalidated_by_with_options(
            graph,
            current_id,
            allow_requirement_to_code,
            allow_requirement_expansion,
        ) {
            if dirty.insert(dependency.derived_id) {
                frontier.push_back(dependency.derived_id);
            }
        }
    }

    dirty.remove(&invalidator_id);
    dirty
}

pub fn invalidation_from_source_changes(
    graph: &ProgramSupergraph,
    artifact_hashes: impl IntoIterator<Item = ArtifactHashChange>,
    stable_subject_ids: impl IntoIterator<Item = InvalidationSubject>,
) -> SourceChangeInvalidation {
    let mut invalidation = SourceChangeInvalidation::default();

    for change in artifact_hashes {
        if artifact_hash_changed(graph, &change) {
            invalidation.changed_artifact_ids.insert(change.artifact_id);
            invalidation
                .direct_dirty_subject_ids
                .insert(change.artifact_id.into());
            let closure = invalidation_closure(graph, change.artifact_id);
            invalidation
                .dirty_subject_ids
                .extend(closure.iter().copied());
            invalidation
                .dirty_generated_view_subject_ids
                .extend(closure);
        }
    }

    for subject_id in stable_subject_ids {
        invalidation.direct_dirty_subject_ids.insert(subject_id);
        invalidation.dirty_subject_ids.insert(subject_id);
        invalidation
            .dirty_generated_view_subject_ids
            .insert(subject_id);
        let closure = invalidation_closure(graph, subject_id);
        invalidation
            .dirty_subject_ids
            .extend(closure.iter().copied());
        invalidation
            .dirty_generated_view_subject_ids
            .extend(closure);
    }

    invalidation
        .dirty_subject_ids
        .extend(invalidation.direct_dirty_subject_ids.iter().copied());
    invalidation
}

fn add_owned_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let invalidator_node = invalidator_id.as_node();
    for node_id in invalidator_node
        .as_ref()
        .and_then(|id| {
            graph
                .indexes
                .owner_to_nodes
                .get(id)
                .or_else(|| graph.indexes.artifact_to_nodes.get(id))
        })
        .into_iter()
        .flatten()
    {
        if node_by_id(graph, *node_id).is_some_and(|node| {
            matches!(
                node.kind,
                NodeKind::Requirement | NodeKind::DomainKnowledge | NodeKind::Diagnostic
            )
        }) {
            continue;
        }
        add_dependency(
            dependencies,
            invalidator_id,
            node_id.into(),
            "owner change dirties owned node",
        );
    }

    for edge_id in invalidator_node
        .as_ref()
        .and_then(|id| graph.indexes.owner_to_edges.get(id))
        .into_iter()
        .flatten()
    {
        if edge_by_id(graph, *edge_id).is_some_and(|edge| {
            matches!(
                edge.kind,
                EdgeKind::TracesTo
                    | EdgeKind::DecomposesTo
                    | EdgeKind::Conditions
                    | EdgeKind::Orders
                    | EdgeKind::DependsOnDomainKnowledge
            )
        }) {
            continue;
        }
        add_dependency(
            dependencies,
            invalidator_id,
            edge_id.into(),
            "owner change dirties owned edge",
        );
    }
}

fn artifact_hash_changed(graph: &ProgramSupergraph, change: &ArtifactHashChange) -> bool {
    match artifact_content_hash(graph, change.artifact_id) {
        Some(existing_hash) => existing_hash != change.new_content_hash,
        None => change.new_content_hash.is_some(),
    }
}

fn artifact_content_hash(graph: &ProgramSupergraph, artifact_id: NodeId) -> Option<Option<String>> {
    node_by_id(graph, artifact_id).and_then(|node| match &node.fact {
        NodeFact::Artifact(artifact) => Some(artifact.content_hash.clone()),
        _ => None,
    })
}

fn add_same_span_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let Some(node) = subject_node(graph, invalidator_id) else {
        return;
    };
    let Some(span) = node.span else {
        return;
    };

    let key = SourceSpanIndexKey {
        artifact_id: node.owner.artifact_id,
        span,
    };
    for same_span_id in graph
        .indexes
        .source_span_to_nodes
        .get(&key)
        .into_iter()
        .flatten()
    {
        if invalidator_id.as_node() != Some(*same_span_id) {
            add_dependency(
                dependencies,
                invalidator_id,
                same_span_id.into(),
                "same source span facts share parser input",
            );
        }
    }
}

fn add_endpoint_edges(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let artifact_seed =
        subject_node(graph, invalidator_id).is_some_and(|node| node.kind == NodeKind::Artifact);
    let invalidator_node = invalidator_id.as_node();
    for edge_id in invalidator_node
        .as_ref()
        .and_then(|id| graph.indexes.outgoing_edges_by_node.get(id))
        .into_iter()
        .flatten()
        .chain(
            invalidator_node
                .as_ref()
                .and_then(|id| graph.indexes.incoming_edges_by_node.get(id))
                .into_iter()
                .flatten(),
        )
    {
        if artifact_seed
            && edge_by_id(graph, *edge_id).is_some_and(|edge| {
                matches!(
                    edge.kind,
                    EdgeKind::TracesTo
                        | EdgeKind::DecomposesTo
                        | EdgeKind::Conditions
                        | EdgeKind::Orders
                        | EdgeKind::DependsOnDomainKnowledge
                )
            })
        {
            continue;
        }
        add_dependency(
            dependencies,
            invalidator_id,
            edge_id.into(),
            "edge endpoint change dirties edge",
        );
    }
}

fn add_trace_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    allow_requirement_to_code: bool,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let invalidator_node = invalidator_id.as_node();
    for requirement_id in invalidator_node
        .as_ref()
        .and_then(|id| graph.indexes.code_to_requirements.get(id))
        .into_iter()
        .flatten()
    {
        add_dependency(
            dependencies,
            invalidator_id,
            requirement_id.into(),
            "code fact and requirement are traced",
        );
    }

    if allow_requirement_to_code {
        for code_fact_id in invalidator_node
            .as_ref()
            .and_then(|id| graph.indexes.requirement_to_code.get(id))
            .into_iter()
            .flatten()
        {
            add_dependency(
                dependencies,
                invalidator_id,
                code_fact_id.into(),
                "requirement and code fact are traced",
            );
        }
    }
}

fn add_domain_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for edge in &graph.edges {
        let EdgeFact::DependsOnDomainKnowledge(dependency) = &edge.fact else {
            continue;
        };
        if invalidator_id.as_node() == Some(dependency.domain_knowledge_id) {
            add_dependency(
                dependencies,
                invalidator_id,
                edge.edge_id.into(),
                "domain knowledge change dirties prose dependency edge",
            );
            add_dependency(
                dependencies,
                invalidator_id,
                dependency.requirement_id.into(),
                "domain knowledge change dirties generated requirement prose",
            );
        }
        if invalidator_id.as_edge() == Some(edge.edge_id) {
            add_dependency(
                dependencies,
                invalidator_id,
                dependency.requirement_id.into(),
                "domain dependency edge dirties generated requirement prose",
            );
        }
    }

    for node in &graph.nodes {
        let NodeFact::Diagnostic(diagnostic) = &node.fact else {
            continue;
        };
        if diagnostic
            .related
            .iter()
            .any(|related| invalidator_id.as_node() == Some(*related))
        {
            add_dependency(
                dependencies,
                invalidator_id,
                diagnostic.diagnostic_id.into(),
                "related diagnostic depends on changed subject",
            );
        }
    }
}

fn add_edge_derived_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let Some(edge) = invalidator_id
        .as_edge()
        .and_then(|edge_id| edge_by_id(graph, edge_id))
    else {
        return;
    };

    match &edge.fact {
        EdgeFact::ResolvesTo(_) => {
            add_call_edges_for_call_site(graph, invalidator_id, edge.source_id, dependencies);
            add_data_flow_edges_for_endpoint(graph, invalidator_id, edge.source_id, dependencies);
            if let Some(target_id) = edge.target_id {
                add_data_flow_edges_for_endpoint(graph, invalidator_id, target_id, dependencies);
            }
        }
        EdgeFact::Calls(calls) => {
            add_interprocedural_edges_for_call_site(
                graph,
                invalidator_id,
                calls.call_site_id,
                dependencies,
            );
        }
        EdgeFact::DataFlow(_)
        | EdgeFact::ParameterIn(_)
        | EdgeFact::ParameterOut(_)
        | EdgeFact::ReturnsTo(_)
        | EdgeFact::ThrowsTo(_) => {
            add_trace_subjects(graph, invalidator_id, false, dependencies);
        }
        EdgeFact::DecomposesTo(_) | EdgeFact::Conditions(_) | EdgeFact::Orders(_) => {
            add_dependency(
                dependencies,
                invalidator_id,
                edge.source_id.into(),
                "requirement relationship edge affects source requirement",
            );
            if let Some(target_id) = edge.target_id {
                add_dependency(
                    dependencies,
                    invalidator_id,
                    target_id.into(),
                    "requirement relationship edge affects target requirement",
                );
            }
        }
        _ => {}
    }
}

fn add_call_edges_for_call_site(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    call_site_id: NodeId,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for edge in edges_by_kind(graph, EdgeKind::Calls) {
        let EdgeFact::Calls(calls) = &edge.fact else {
            continue;
        };
        if calls.call_site_id == call_site_id {
            add_dependency(
                dependencies,
                invalidator_id,
                edge.edge_id.into(),
                "call edge depends on call-site resolution",
            );
        }
    }
}

fn add_interprocedural_edges_for_call_site(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    call_site_id: NodeId,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for edge in graph.edges.iter().filter(|edge| {
        matches!(
            edge.kind,
            EdgeKind::ParameterIn
                | EdgeKind::ParameterOut
                | EdgeKind::ReturnsTo
                | EdgeKind::ThrowsTo
        )
    }) {
        let call_site_matches = match &edge.fact {
            EdgeFact::ParameterIn(parameter) => parameter.call_site_id == call_site_id,
            EdgeFact::ParameterOut(parameter) => parameter.call_site_id == call_site_id,
            EdgeFact::ReturnsTo(returns) => returns.call_site_id == call_site_id,
            EdgeFact::ThrowsTo(throws) => throws.call_site_id == call_site_id,
            _ => false,
        };
        if call_site_matches {
            add_dependency(
                dependencies,
                invalidator_id,
                edge.edge_id.into(),
                "interprocedural edge depends on resolved call",
            );
        }
    }
}

fn add_data_flow_edges_for_endpoint(
    graph: &ProgramSupergraph,
    invalidator_id: InvalidationSubject,
    endpoint_id: NodeId,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for edge in [
        EdgeKind::Uses,
        EdgeKind::Defines,
        EdgeKind::DataFlow,
        EdgeKind::ParameterIn,
        EdgeKind::ParameterOut,
        EdgeKind::ReturnsTo,
        EdgeKind::ThrowsTo,
    ]
    .into_iter()
    .flat_map(|kind| {
        graph
            .indexes
            .outgoing_edges_by_node_and_kind
            .get(&endpoint_id)
            .and_then(|edges_by_kind| edges_by_kind.get(&kind))
            .into_iter()
            .flatten()
            .chain(
                graph
                    .indexes
                    .incoming_edges_by_node_and_kind
                    .get(&endpoint_id)
                    .and_then(|edges_by_kind| edges_by_kind.get(&kind))
                    .into_iter()
                    .flatten(),
            )
    }) {
        add_dependency(
            dependencies,
            invalidator_id,
            edge.into(),
            "data-flow fact depends on resolved symbol",
        );
    }
}

fn edges_by_kind(graph: &ProgramSupergraph, kind: EdgeKind) -> impl Iterator<Item = &GraphEdge> {
    graph
        .indexes
        .edges_by_kind
        .get(&kind)
        .into_iter()
        .flatten()
        .filter_map(|edge_id| edge_by_id(graph, *edge_id))
}

fn subject_node(graph: &ProgramSupergraph, subject: InvalidationSubject) -> Option<&GraphNode> {
    subject.as_node().and_then(|id| node_by_id(graph, id))
}

fn node_by_id(graph: &ProgramSupergraph, node_id: NodeId) -> Option<&GraphNode> {
    graph
        .indexes
        .node_position_by_id
        .get(&node_id)
        .and_then(|position| graph.nodes.get(*position))
}

fn edge_by_id(graph: &ProgramSupergraph, edge_id: EdgeId) -> Option<&GraphEdge> {
    graph
        .indexes
        .edge_position_by_id
        .get(&edge_id)
        .and_then(|position| graph.edges.get(*position))
}

fn add_dependency(
    dependencies: &mut BTreeSet<InvalidationDependency>,
    invalidator_id: InvalidationSubject,
    derived_id: InvalidationSubject,
    reason: &'static str,
) {
    if invalidator_id == derived_id {
        return;
    }
    dependencies.insert(InvalidationDependency {
        invalidator_id,
        derived_id,
        reason,
    });
}
