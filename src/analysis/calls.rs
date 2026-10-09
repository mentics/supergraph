use crate::intern::Sym;
use std::collections::{BTreeMap, BTreeSet};

use crate::supergraph::{
    self as sg, CallEdgeKind, Confidence, DispatchKind, EdgeFact, EdgeKind, GraphEdge, NodeFact,
    NodeId, NodeKind, ProgramSupergraph, Resolution, uncertainty_from_resolution,
};

use super::{edge_id, inference_evidence, insert_edge};

const PRECISION: &str = "sg040-call-edge-lowering";

pub(crate) fn emit(graph: &mut ProgramSupergraph) {
    let snapshot = CallResolutionSnapshot::new(graph);
    graph
        .edges
        .retain(|edge| !matches!(edge.fact, EdgeFact::Calls(_)));

    let mut emitted = BTreeSet::new();
    for resolution_edge in &snapshot.call_site_resolution_edges {
        let EdgeFact::ResolvesTo(resolves_to) = &resolution_edge.fact else {
            continue;
        };
        let Some(call_site) = snapshot.call_sites.get(&resolution_edge.source_id) else {
            continue;
        };
        let Some(target_id) = resolution_edge.target_id.as_ref() else {
            continue;
        };
        let Some(target_kind) = snapshot.node_kinds.get(target_id).copied() else {
            continue;
        };

        let Some(call_target) =
            call_target_from_resolution(call_site, *target_id, target_kind, resolves_to.resolution)
        else {
            continue;
        };
        let target_key = call_target.edge_id_target_key();
        let key = (
            call_site.call_site_id.clone(),
            target_key.clone(),
            resolves_to.resolution,
        );
        if !emitted.insert(key) {
            continue;
        }

        let mut evidence = resolution_edge.evidence.clone();
        evidence.extend(inference_evidence(
            "call edge lowered from call-site resolution",
        ));

        insert_edge(
            graph,
            GraphEdge {
                edge_id: edge_id(
                    "calls",
                    call_site.call_site_id,
                    target_key.part(),
                    &format!("{PRECISION}:{:?}", resolves_to.resolution),
                ),
                fact_id: None,
                payload_hash: None,
                kind: EdgeKind::Calls,
                source_id: call_site.call_site_id.clone(),
                target_id: call_target.edge_target_id.clone(),
                owner: resolution_edge.owner.clone(),
                span: resolution_edge.span.or(Some(call_site.span)),
                confidence: confidence_from_resolution(resolves_to.resolution),
                uncertainty: uncertainty_from_resolution(resolves_to.resolution),
                evidence,
                fact: EdgeFact::Calls(sg::Calls {
                    caller_callable_id: call_site.enclosing_callable_id.clone(),
                    callee_callable_id: call_target.callee_callable_id,
                    external_target_id: call_target.external_target_id,
                    unresolved_target: (call_target.unresolved_target).map(Sym::from),
                    call_site_id: call_site.call_site_id.clone(),
                    kind: call_kind(call_site, target_kind, resolves_to.resolution),
                    resolution: resolves_to.resolution,
                }),
            },
        );
    }
}

#[derive(Debug)]
struct CallResolutionSnapshot {
    call_sites: BTreeMap<NodeId, sg::CallSite>,
    node_kinds: BTreeMap<NodeId, NodeKind>,
    call_site_resolution_edges: Vec<GraphEdge>,
}

impl CallResolutionSnapshot {
    fn new(graph: &ProgramSupergraph) -> Self {
        let mut call_sites = BTreeMap::new();
        let mut node_kinds = BTreeMap::new();
        for node in &graph.nodes {
            node_kinds.insert(node.node_id.clone(), node.kind);
            if let NodeFact::CallSite(call_site) = &node.fact {
                call_sites.insert(call_site.call_site_id.clone(), call_site.as_ref().clone());
            }
        }

        let mut call_site_resolution_edges = Vec::new();
        for edge in &graph.edges {
            if matches!(edge.fact, EdgeFact::ResolvesTo(_))
                && call_sites.contains_key(&edge.source_id)
            {
                call_site_resolution_edges.push(edge.clone());
            }
        }

        Self {
            call_sites,
            node_kinds,
            call_site_resolution_edges,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TargetKey {
    Id(NodeId),
    Text(String),
}

impl TargetKey {
    fn part(&self) -> sg::ids::IdPart<'_> {
        match self {
            TargetKey::Id(id) => sg::ids::IdPart::Id(*id),
            TargetKey::Text(text) => sg::ids::IdPart::Str(text),
        }
    }
}

struct CallTarget {
    resolution_target_id: NodeId,
    edge_target_id: Option<NodeId>,
    callee_callable_id: Option<NodeId>,
    external_target_id: Option<NodeId>,
    unresolved_target: Option<String>,
}

impl CallTarget {
    fn edge_id_target_key(&self) -> TargetKey {
        match (&self.edge_target_id, &self.unresolved_target) {
            (Some(id), _) => TargetKey::Id(*id),
            (None, Some(text)) => TargetKey::Text(text.clone()),
            (None, None) => TargetKey::Id(self.resolution_target_id),
        }
    }
}

fn call_target_from_resolution(
    call_site: &sg::CallSite,
    target_id: NodeId,
    target_kind: NodeKind,
    resolution: Resolution,
) -> Option<CallTarget> {
    match target_kind {
        NodeKind::Callable => Some(CallTarget {
            resolution_target_id: target_id,
            edge_target_id: Some(target_id),
            callee_callable_id: Some(target_id),
            external_target_id: None,
            unresolved_target: None,
        }),
        NodeKind::ExternalTarget => Some(CallTarget {
            resolution_target_id: target_id,
            edge_target_id: (resolution != Resolution::Unresolved).then(|| target_id),
            callee_callable_id: None,
            external_target_id: (resolution != Resolution::Unresolved)
                .then(|| target_id),
            unresolved_target: ((resolution == Resolution::Unresolved)
                .then(|| call_site.callee_expression.clone())).map(|sym| sym.to_string()),
        }),
        NodeKind::Binding => Some(CallTarget {
            resolution_target_id: target_id,
            edge_target_id: None,
            callee_callable_id: None,
            external_target_id: None,
            unresolved_target: Some((call_site.callee_expression.clone()).to_string()),
        }),
        _ => None,
    }
}

fn call_kind(
    call_site: &sg::CallSite,
    target_kind: NodeKind,
    resolution: Resolution,
) -> CallEdgeKind {
    if matches!(target_kind, NodeKind::Binding)
        || matches!(resolution, Resolution::Possible | Resolution::Unresolved)
    {
        return CallEdgeKind::PossibleDynamic;
    }
    if resolution == Resolution::External {
        return CallEdgeKind::External;
    }
    match call_site.dispatch_kind {
        DispatchKind::Direct => CallEdgeKind::Direct,
        DispatchKind::Method => CallEdgeKind::Method,
        DispatchKind::Constructor => CallEdgeKind::Constructor,
        DispatchKind::Decorator => CallEdgeKind::Decorator,
        DispatchKind::HigherOrder | DispatchKind::Unknown => CallEdgeKind::PossibleDynamic,
    }
}

fn confidence_from_resolution(resolution: Resolution) -> Confidence {
    match uncertainty_from_resolution(resolution) {
        sg::Uncertainty::Exact | sg::Uncertainty::External => Confidence::Exact,
        sg::Uncertainty::Probable | sg::Uncertainty::Possible | sg::Uncertainty::Ambiguous => {
            Confidence::Probable
        }
        sg::Uncertainty::Unresolved | sg::Uncertainty::Unsupported | sg::Uncertainty::Stale => {
            Confidence::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::supergraph::ids::test_support::{test_edge_id, test_id};
    use super::*;
    use crate::ast::{CallContext, SourceSpan};
    use crate::supergraph::{
        Binding, BindingKind, BindingTarget, ExternalTarget, ExternalTargetKind, GraphIndexes,
        GraphNode, SCHEMA_VERSION, SourceOwnership, Uncertainty,
    };

    #[test]
    fn emits_calls_from_call_site_resolutions_only() {
        let mut graph = graph_with_nodes(vec![
            callable_node("caller"),
            callable_node("callee"),
            binding_node("dynamic-binding", "dynamic"),
            external_node("external-target", "pkg.external"),
            external_node("unresolved-placeholder", "missing_call"),
            call_site_node("call-exact", "callee", DispatchKind::Direct),
            call_site_node("call-external", "pkg.external", DispatchKind::Method),
            call_site_node("call-dynamic", "dynamic", DispatchKind::Direct),
            call_site_node("call-unresolved", "missing_call", DispatchKind::Direct),
        ]);
        graph.edges = vec![
            resolves_to_edge("call-exact", "callee", Resolution::Exact),
            resolves_to_edge("call-external", "external-target", Resolution::External),
            resolves_to_edge("call-dynamic", "dynamic-binding", Resolution::Possible),
            resolves_to_edge(
                "call-unresolved",
                "unresolved-placeholder",
                Resolution::Unresolved,
            ),
            stale_call_edge(
                "stale-call",
                "call-exact",
                Some("callee"),
                CallEdgeKind::PossibleDynamic,
                Resolution::Possible,
            ),
        ];

        emit(&mut graph);

        let calls = call_edges(&graph);
        assert_eq!(
            calls.len(),
            4,
            "only canonical call-site resolution edges should emit Calls edges"
        );
        assert_call(
            &graph,
            "call-exact",
            Some("callee"),
            Some("callee"),
            None,
            None,
            CallEdgeKind::Direct,
            Resolution::Exact,
        );
        assert_call(
            &graph,
            "call-external",
            Some("external-target"),
            None,
            Some("external-target"),
            None,
            CallEdgeKind::External,
            Resolution::External,
        );
        assert_call(
            &graph,
            "call-dynamic",
            None,
            None,
            None,
            Some("dynamic"),
            CallEdgeKind::PossibleDynamic,
            Resolution::Possible,
        );
        assert_call(
            &graph,
            "call-unresolved",
            None,
            None,
            None,
            Some("missing_call"),
            CallEdgeKind::PossibleDynamic,
            Resolution::Unresolved,
        );
        assert!(
            calls.iter().all(|edge| edge.edge_id != test_edge_id("stale-call")),
            "pre-existing Calls edges should not be preserved without current resolution facts"
        );
    }

    #[test]
    fn drops_calls_without_canonical_call_site_resolution() {
        let mut graph = graph_with_nodes(vec![
            callable_node("caller"),
            callable_node("callee"),
            call_site_node("call-without-resolution", "callee", DispatchKind::Direct),
        ]);
        graph.edges = vec![stale_call_edge(
            "stale-call",
            "call-without-resolution",
            Some("callee"),
            CallEdgeKind::Direct,
            Resolution::Exact,
        )];

        emit(&mut graph);

        assert!(
            call_edges(&graph).is_empty(),
            "Calls edges require current CallSite plus ResolvesTo facts"
        );
    }

    fn graph_with_nodes(nodes: Vec<GraphNode>) -> ProgramSupergraph {
        ProgramSupergraph {
            schema_version: SCHEMA_VERSION.to_string(),
            language: "test".to_string(),
            root: String::new(),
            nodes,
            edges: Vec::new(),
            indexes: GraphIndexes::default(),
            id_cache: Default::default(),
        }
    }

    fn callable_node(id: &str) -> GraphNode {
        node(
            id,
            NodeKind::Callable,
            NodeFact::Callable(Box::new(sg::Callable {
                callable_id: test_id(id),
                kind: sg::CallableKind::Function,
                name: Some(Sym::from(id.to_string())),
                qualified_name: Sym::from(id.to_string()),
                artifact_id: test_id("artifact"),
                declaration_span: span(),
                body_span: Some(span()),
                signature: sg::Signature {
                    parameters: Vec::new(),
                    return_annotation: None,
                },
                scope_id: test_id("scope"),
                attributes: Vec::new(),
                incoming_local_call_count: 0,
                external_invocation_metadata: Vec::new(),
            })),
        )
    }

    fn binding_node(id: &str, name: &str) -> GraphNode {
        node(
            id,
            NodeKind::Binding,
            NodeFact::Binding(Binding {
                binding_id: test_id(id),
                scope_id: test_id("scope"),
                name: Sym::from(name.to_string()),
                kind: BindingKind::Assignment,
                target: BindingTarget::Value(Sym::from(name.to_string())),
                span: span(),
            }),
        )
    }

    fn external_node(id: &str, qualified_name: &str) -> GraphNode {
        node(
            id,
            NodeKind::ExternalTarget,
            NodeFact::ExternalTarget(ExternalTarget {
                external_target_id: test_id(id),
                ecosystem: Sym::from("test".to_string()),
                package_name: None,
                package_version: None,
                module_path: None,
                qualified_name: Sym::from(qualified_name.to_string()),
                member_path: None,
                target_kind: ExternalTargetKind::Unknown,
                source: Sym::from("test".to_string()),
            }),
        )
    }

    fn call_site_node(id: &str, callee: &str, dispatch_kind: DispatchKind) -> GraphNode {
        node(
            id,
            NodeKind::CallSite,
            NodeFact::CallSite(Box::new(sg::CallSite {
                call_site_id: test_id(id),
                artifact_id: test_id("artifact"),
                enclosing_callable_id: test_id("caller"),
                span: span(),
                callee_expression: Sym::from(callee.to_string()),
                argument_shape: sg::ArgumentShape {
                    positional_count: 0,
                    named_arguments: Vec::new(),
                },
                dispatch_kind,
                context: CallContext::Body,
            })),
        )
    }

    fn node(id: &str, kind: NodeKind, fact: NodeFact) -> GraphNode {
        GraphNode {
            node_id: test_id(id),
            fact_id: None,
            payload_hash: None,
            kind,
            owner: SourceOwnership::default(),
            span: Some(span()),
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: Vec::new(),
            fact,
        }
    }

    fn resolves_to_edge(source_id: &str, target_id: &str, resolution: Resolution) -> GraphEdge {
        GraphEdge {
            edge_id: test_edge_id(&format!("resolves:{source_id}:{target_id}:{resolution:?}")),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::ResolvesTo,
            source_id: test_id(source_id),
            target_id: Some(test_id(target_id)),
            owner: SourceOwnership {
                artifact_id: Some(test_id("artifact")),
                scope_id: None,
                callable_id: Some(test_id("caller")),
            },
            span: Some(span()),
            confidence: confidence_from_resolution(resolution),
            uncertainty: uncertainty_from_resolution(resolution),
            evidence: Vec::new(),
            fact: EdgeFact::ResolvesTo(sg::ResolvesTo {
                binding_id: test_id(source_id),
                target_id: test_id(target_id),
                resolution,
            }),
        }
    }

    fn stale_call_edge(
        edge_id: &str,
        call_site_id: &str,
        target_id: Option<&str>,
        kind: CallEdgeKind,
        resolution: Resolution,
    ) -> GraphEdge {
        GraphEdge {
            edge_id: test_edge_id(edge_id),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::Calls,
            source_id: test_id(call_site_id),
            target_id: target_id.map(test_id),
            owner: SourceOwnership::default(),
            span: Some(span()),
            confidence: confidence_from_resolution(resolution),
            uncertainty: uncertainty_from_resolution(resolution),
            evidence: Vec::new(),
            fact: EdgeFact::Calls(sg::Calls {
                caller_callable_id: test_id("caller"),
                callee_callable_id: target_id.map(test_id),
                external_target_id: None,
                unresolved_target: None,
                call_site_id: test_id(call_site_id),
                kind,
                resolution,
            }),
        }
    }

    fn assert_call(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        edge_target_id: Option<&str>,
        callee_callable_id: Option<&str>,
        external_target_id: Option<&str>,
        unresolved_target: Option<&str>,
        kind: CallEdgeKind,
        resolution: Resolution,
    ) {
        assert!(
            call_edges(graph).iter().any(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::Calls(calls)
                        if calls.call_site_id == test_id(call_site_id)
                            && edge.source_id == test_id(call_site_id)
                            && edge.target_id == edge_target_id.map(test_id)
                            && calls.callee_callable_id == callee_callable_id.map(test_id)
                            && calls.external_target_id == external_target_id.map(test_id)
                            && calls.unresolved_target.as_deref() == unresolved_target
                            && calls.kind == kind
                            && calls.resolution == resolution
                )
            }),
            "missing {resolution:?} call for {call_site_id}"
        );
    }

    fn call_edges(graph: &ProgramSupergraph) -> Vec<&GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| matches!(edge.fact, EdgeFact::Calls(_)))
            .collect()
    }

    fn span() -> SourceSpan {
        SourceSpan {
            start_byte: 0,
            end_byte: 1,
            start_row: 0,
            start_column: 0,
            end_row: 0,
            end_column: 1,
        }
    }
}
