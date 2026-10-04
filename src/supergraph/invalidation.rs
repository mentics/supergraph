use std::collections::{BTreeSet, VecDeque};

use super::{
    EdgeFact, EdgeKind, GraphEdge, GraphNode, NodeFact, NodeId, NodeKind, ProgramSupergraph,
    SourceSpanIndexKey,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InvalidationDependency {
    pub invalidator_id: NodeId,
    pub derived_id: NodeId,
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
    pub direct_dirty_subject_ids: BTreeSet<NodeId>,
    pub dirty_subject_ids: BTreeSet<NodeId>,
    pub dirty_generated_view_subject_ids: BTreeSet<NodeId>,
}

pub fn invalidated_by(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
) -> Vec<InvalidationDependency> {
    invalidated_by_with_options(graph, invalidator_id, true, true)
}

fn invalidated_by_with_options(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    allow_requirement_to_code: bool,
    allow_requirement_expansion: bool,
) -> Vec<InvalidationDependency> {
    let mut dependencies = BTreeSet::new();
    if !allow_requirement_expansion
        && node_by_id(graph, invalidator_id).is_some_and(|node| node.kind == NodeKind::Requirement)
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

pub fn invalidation_closure(graph: &ProgramSupergraph, invalidator_id: &str) -> BTreeSet<NodeId> {
    let mut dirty = BTreeSet::new();
    let mut frontier = VecDeque::from([invalidator_id.to_string()]);

    while let Some(current_id) = frontier.pop_front() {
        let allow_requirement_to_code = current_id == invalidator_id;
        let allow_requirement_expansion = current_id == invalidator_id;
        for dependency in invalidated_by_with_options(
            graph,
            &current_id,
            allow_requirement_to_code,
            allow_requirement_expansion,
        ) {
            if dirty.insert(dependency.derived_id.clone()) {
                frontier.push_back(dependency.derived_id);
            }
        }
    }

    dirty.remove(invalidator_id);
    dirty
}

pub fn invalidation_from_source_changes(
    graph: &ProgramSupergraph,
    artifact_hashes: impl IntoIterator<Item = ArtifactHashChange>,
    stable_subject_ids: impl IntoIterator<Item = NodeId>,
) -> SourceChangeInvalidation {
    let mut invalidation = SourceChangeInvalidation::default();

    for change in artifact_hashes {
        if artifact_hash_changed(graph, &change) {
            invalidation
                .changed_artifact_ids
                .insert(change.artifact_id.clone());
            invalidation
                .direct_dirty_subject_ids
                .insert(change.artifact_id.clone());
            let closure = invalidation_closure(graph, &change.artifact_id);
            invalidation
                .dirty_subject_ids
                .extend(closure.iter().cloned());
            invalidation
                .dirty_generated_view_subject_ids
                .extend(closure);
        }
    }

    for subject_id in stable_subject_ids {
        invalidation
            .direct_dirty_subject_ids
            .insert(subject_id.clone());
        invalidation.dirty_subject_ids.insert(subject_id.clone());
        invalidation
            .dirty_generated_view_subject_ids
            .insert(subject_id.clone());
        let closure = invalidation_closure(graph, &subject_id);
        invalidation
            .dirty_subject_ids
            .extend(closure.iter().cloned());
        invalidation
            .dirty_generated_view_subject_ids
            .extend(closure);
    }

    invalidation
        .dirty_subject_ids
        .extend(invalidation.direct_dirty_subject_ids.iter().cloned());
    invalidation
}

fn add_owned_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for node_id in graph
        .indexes
        .owner_to_nodes
        .get(invalidator_id)
        .or_else(|| graph.indexes.artifact_to_nodes.get(invalidator_id))
        .into_iter()
        .flatten()
    {
        if node_by_id(graph, node_id).is_some_and(|node| {
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
            node_id,
            "owner change dirties owned node",
        );
    }

    for edge_id in graph
        .indexes
        .owner_to_edges
        .get(invalidator_id)
        .into_iter()
        .flatten()
    {
        if edge_by_id(graph, edge_id).is_some_and(|edge| {
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
            edge_id,
            "owner change dirties owned edge",
        );
    }
}

fn artifact_hash_changed(graph: &ProgramSupergraph, change: &ArtifactHashChange) -> bool {
    match artifact_content_hash(graph, &change.artifact_id) {
        Some(existing_hash) => existing_hash != change.new_content_hash,
        None => change.new_content_hash.is_some(),
    }
}

fn artifact_content_hash(graph: &ProgramSupergraph, artifact_id: &str) -> Option<Option<String>> {
    node_by_id(graph, artifact_id).and_then(|node| match &node.fact {
        NodeFact::Artifact(artifact) => Some(artifact.content_hash.clone()),
        _ => None,
    })
}

fn add_same_span_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let Some(node) = node_by_id(graph, invalidator_id) else {
        return;
    };
    let Some(span) = node.span else {
        return;
    };

    let key = SourceSpanIndexKey {
        artifact_id: node.owner.artifact_id.clone(),
        span,
    };
    for same_span_id in graph
        .indexes
        .source_span_to_nodes
        .get(&key)
        .into_iter()
        .flatten()
    {
        if same_span_id != invalidator_id {
            add_dependency(
                dependencies,
                invalidator_id,
                same_span_id,
                "same source span facts share parser input",
            );
        }
    }
}

fn add_endpoint_edges(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let artifact_seed =
        node_by_id(graph, invalidator_id).is_some_and(|node| node.kind == NodeKind::Artifact);
    for edge_id in graph
        .indexes
        .outgoing_edges_by_node
        .get(invalidator_id)
        .into_iter()
        .flatten()
        .chain(
            graph
                .indexes
                .incoming_edges_by_node
                .get(invalidator_id)
                .into_iter()
                .flatten(),
        )
    {
        if artifact_seed
            && edge_by_id(graph, edge_id).is_some_and(|edge| {
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
            edge_id,
            "edge endpoint change dirties edge",
        );
    }
}

fn add_trace_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    allow_requirement_to_code: bool,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for requirement_id in graph
        .indexes
        .code_to_requirements
        .get(invalidator_id)
        .into_iter()
        .flatten()
    {
        add_dependency(
            dependencies,
            invalidator_id,
            requirement_id,
            "code fact and requirement are traced",
        );
    }

    if allow_requirement_to_code {
        for code_fact_id in graph
            .indexes
            .requirement_to_code
            .get(invalidator_id)
            .into_iter()
            .flatten()
        {
            add_dependency(
                dependencies,
                invalidator_id,
                code_fact_id,
                "requirement and code fact are traced",
            );
        }
    }
}

fn add_domain_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    for edge in &graph.edges {
        let EdgeFact::DependsOnDomainKnowledge(dependency) = &edge.fact else {
            continue;
        };
        if dependency.domain_knowledge_id == invalidator_id {
            add_dependency(
                dependencies,
                invalidator_id,
                &edge.edge_id,
                "domain knowledge change dirties prose dependency edge",
            );
            add_dependency(
                dependencies,
                invalidator_id,
                &dependency.requirement_id,
                "domain knowledge change dirties generated requirement prose",
            );
        }
        if edge.edge_id == invalidator_id {
            add_dependency(
                dependencies,
                invalidator_id,
                &dependency.requirement_id,
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
            .any(|related| related == invalidator_id)
        {
            add_dependency(
                dependencies,
                invalidator_id,
                &diagnostic.diagnostic_id,
                "related diagnostic depends on changed subject",
            );
        }
    }
}

fn add_edge_derived_subjects(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    dependencies: &mut BTreeSet<InvalidationDependency>,
) {
    let Some(edge) = edge_by_id(graph, invalidator_id) else {
        return;
    };

    match &edge.fact {
        EdgeFact::ResolvesTo(_) => {
            add_call_edges_for_call_site(graph, invalidator_id, &edge.source_id, dependencies);
            add_data_flow_edges_for_endpoint(graph, invalidator_id, &edge.source_id, dependencies);
            if let Some(target_id) = &edge.target_id {
                add_data_flow_edges_for_endpoint(graph, invalidator_id, target_id, dependencies);
            }
        }
        EdgeFact::Calls(calls) => {
            add_interprocedural_edges_for_call_site(
                graph,
                invalidator_id,
                &calls.call_site_id,
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
                &edge.source_id,
                "requirement relationship edge affects source requirement",
            );
            if let Some(target_id) = &edge.target_id {
                add_dependency(
                    dependencies,
                    invalidator_id,
                    target_id,
                    "requirement relationship edge affects target requirement",
                );
            }
        }
        _ => {}
    }
}

fn add_call_edges_for_call_site(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    call_site_id: &str,
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
                &edge.edge_id,
                "call edge depends on call-site resolution",
            );
        }
    }
}

fn add_interprocedural_edges_for_call_site(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    call_site_id: &str,
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
                &edge.edge_id,
                "interprocedural edge depends on resolved call",
            );
        }
    }
}

fn add_data_flow_edges_for_endpoint(
    graph: &ProgramSupergraph,
    invalidator_id: &str,
    endpoint_id: &str,
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
            .get(endpoint_id)
            .and_then(|edges_by_kind| edges_by_kind.get(&kind))
            .into_iter()
            .flatten()
            .chain(
                graph
                    .indexes
                    .incoming_edges_by_node_and_kind
                    .get(endpoint_id)
                    .and_then(|edges_by_kind| edges_by_kind.get(&kind))
                    .into_iter()
                    .flatten(),
            )
    }) {
        add_dependency(
            dependencies,
            invalidator_id,
            edge,
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
        .filter_map(|edge_id| edge_by_id(graph, edge_id))
}

fn node_by_id<'a>(graph: &'a ProgramSupergraph, node_id: &str) -> Option<&'a GraphNode> {
    graph
        .indexes
        .node_position_by_id
        .get(node_id)
        .and_then(|position| graph.nodes.get(*position))
}

fn edge_by_id<'a>(graph: &'a ProgramSupergraph, edge_id: &str) -> Option<&'a GraphEdge> {
    graph
        .indexes
        .edge_position_by_id
        .get(edge_id)
        .and_then(|position| graph.edges.get(*position))
}

fn add_dependency(
    dependencies: &mut BTreeSet<InvalidationDependency>,
    invalidator_id: &str,
    derived_id: &str,
    reason: &'static str,
) {
    if invalidator_id == derived_id {
        return;
    }
    dependencies.insert(InvalidationDependency {
        invalidator_id: invalidator_id.to_string(),
        derived_id: derived_id.to_string(),
        reason,
    });
}
