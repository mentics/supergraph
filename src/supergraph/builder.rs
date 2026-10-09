use crate::intern::Sym;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    io::{self, Write},
};

use crate::ast::SourceSpan;
use serde::{Serialize, Serializer, ser::SerializeMap};

use super::multimap::MultiMap;
use super::ids::{
    EdgeId, FactId, IdMap, IdPart, PayloadHash, Tag, callable_id_from_text, with_legacy_id_text,
};
use crate::id_parts;
use std::num::NonZeroU64;

use super::schema::{
    Artifact, BasicBlock, Binding, Binds, CallSite, Callable, Calls, Condition, Conditions,
    Confidence, Contains, ControlFlow, Controls, DataFlow, DecomposesTo, Defines, Definition,
    DependsOnDomainKnowledge, Diagnostic, DomainKnowledge, EdgeFact, EdgeKind, Evidence,
    Expression, ExternalTarget, GraphEdge, GraphIndexes, GraphNode, NodeFact, NodeId, NodeKind,
    Orders, ParameterIn, ParameterOut, ProgramSupergraph, ResolvesTo, ReturnsTo, SCHEMA_VERSION,
    SourceOwnership, SourceSpanIndexKey, Statement, Symbol, SyntaxReference, ThrowsTo,
    TracesTo, Uncertainty, Use, Uses, Value, uncertainty_from_confidence,
    uncertainty_from_diagnostic_kind, uncertainty_from_domain_knowledge_status,
    uncertainty_from_resolution,
};

#[derive(Debug, Clone)]
pub struct ProgramSupergraphBuilder {
    root: String,
    language: String,
    nodes: BTreeMap<NodeId, GraphNode>,
    edges: BTreeMap<EdgeId, GraphEdge>,
}

impl ProgramSupergraphBuilder {
    pub fn new(root: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            language: language.into(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
        }
    }

    pub fn insert_node(&mut self, mut node: GraphNode) -> bool {
        node.uncertainty = node_uncertainty(&node.confidence, &node.fact);
        sort_evidence(&mut node.evidence);
        match self.nodes.entry(node.node_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(node);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn insert_edge(&mut self, mut edge: GraphEdge) -> bool {
        edge.uncertainty = edge_uncertainty(&edge.confidence, &edge.fact);
        sort_evidence(&mut edge.evidence);
        match self.edges.entry(edge.edge_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(edge);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn source_evidence(
        &self,
        kind: super::schema::EvidenceKind,
        summary: impl Into<String>,
        source_id: impl Into<Option<NodeId>>,
        source_span: impl Into<Option<SourceSpan>>,
        content_hash: impl Into<Option<String>>,
        syntax: impl Into<Option<SyntaxReference>>,
    ) -> Evidence {
        Evidence {
            kind,
            summary: Sym::from(summary.into()),
            source_id: source_id.into(),
            source_span: source_span.into(),
            content_hash: (content_hash.into()).map(Sym::from),
            syntax: syntax.into(),
        }
    }

    pub fn add_artifact(&mut self, artifact: Artifact, evidence: Vec<Evidence>) -> NodeId {
        let node_id = artifact.artifact_id.clone();
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Artifact,
            owner: SourceOwnership {
                artifact_id: Some(node_id.clone()),
                scope_id: None,
                callable_id: None,
            },
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::Artifact(artifact),
        });
        node_id
    }

    pub fn add_scope(
        &mut self,
        scope: super::schema::Scope,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = scope.scope_id.clone();
        let owner = SourceOwnership {
            artifact_id: Some(scope.artifact_id.clone()),
            scope_id: scope.parent_scope_id.clone(),
            callable_id: scope.owner_callable_id.clone(),
        };
        let span = scope.span;
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Scope,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::Scope(Box::new(scope)),
        });
        node_id
    }

    pub fn add_binding(
        &mut self,
        binding: Binding,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = binding.binding_id.clone();
        let owner = SourceOwnership {
            artifact_id: None,
            scope_id: Some(binding.scope_id.clone()),
            callable_id: None,
        };
        let span = Some(binding.span);
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Binding,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::Binding(binding),
        });
        node_id
    }

    pub fn add_callable(
        &mut self,
        callable: Callable,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = callable.callable_id.clone();
        let owner = SourceOwnership {
            artifact_id: Some(callable.artifact_id.clone()),
            scope_id: Some(callable.scope_id.clone()),
            callable_id: Some(node_id.clone()),
        };
        let span = Some(callable.declaration_span);
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Callable,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::Callable(Box::new(callable)),
        });
        node_id
    }

    pub(crate) fn update_callables(&mut self, mut update: impl FnMut(&mut Callable)) {
        for node in self.nodes.values_mut() {
            if let NodeFact::Callable(callable) = &mut node.fact {
                update(callable);
            }
        }
    }

    pub fn add_call_site(
        &mut self,
        call_site: CallSite,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = call_site.call_site_id.clone();
        let owner = SourceOwnership {
            artifact_id: Some(call_site.artifact_id.clone()),
            scope_id: None,
            callable_id: Some(call_site.enclosing_callable_id.clone()),
        };
        let span = Some(call_site.span);
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::CallSite,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::CallSite(Box::new(call_site)),
        });
        node_id
    }

    pub fn add_external_target(
        &mut self,
        target: ExternalTarget,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = target.external_target_id.clone();
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::ExternalTarget,
            owner: SourceOwnership::default(),
            span: None,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::ExternalTarget(target),
        });
        node_id
    }

    pub fn add_statement(
        &mut self,
        statement: Statement,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = statement.statement_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Statement,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Statement(Box::new(statement)),
        )
    }

    pub fn add_expression(
        &mut self,
        expression: Expression,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = expression.expression_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Expression,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Expression(Box::new(expression)),
        )
    }

    pub fn add_condition(
        &mut self,
        condition: Condition,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = condition.condition_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Condition,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Condition(Box::new(condition)),
        )
    }

    pub fn add_symbol(
        &mut self,
        symbol: Symbol,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = symbol.symbol_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Symbol,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Symbol(symbol),
        )
    }

    pub fn add_definition(
        &mut self,
        definition: Definition,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = definition.definition_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Definition,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Definition(definition),
        )
    }

    pub fn add_use(
        &mut self,
        use_fact: Use,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = use_fact.use_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Use,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Use(use_fact),
        )
    }

    pub fn add_value(
        &mut self,
        value: Value,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = value.value_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::Value,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::Value(value),
        )
    }

    pub fn add_basic_block(
        &mut self,
        basic_block: BasicBlock,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = basic_block.basic_block_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::BasicBlock,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::BasicBlock(Box::new(basic_block)),
        )
    }

    pub fn add_domain_knowledge(
        &mut self,
        domain_knowledge: DomainKnowledge,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = domain_knowledge.domain_knowledge_id.clone();
        self.add_structural_node(
            node_id,
            NodeKind::DomainKnowledge,
            owner,
            span,
            confidence,
            evidence,
            NodeFact::DomainKnowledge(Box::new(domain_knowledge)),
        )
    }

    pub fn add_diagnostic(
        &mut self,
        diagnostic: Diagnostic,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> NodeId {
        let node_id = diagnostic.diagnostic_id.clone();
        let owner = SourceOwnership {
            artifact_id: diagnostic.artifact_id.clone(),
            scope_id: None,
            callable_id: None,
        };
        let span = diagnostic.span;
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Diagnostic,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: NodeFact::Diagnostic(diagnostic),
        });
        node_id
    }

    pub fn add_contains(
        &mut self,
        container_id: NodeId,
        member_id: NodeId,
        owner: SourceOwnership,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let edge_id = structural_edge_id("contains", container_id, member_id);
        self.insert_edge(GraphEdge {
            edge_id: edge_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::Contains,
            source_id: container_id.clone(),
            target_id: Some(member_id.clone()),
            owner,
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: EdgeFact::Contains(Contains {
                container_id,
                member_id,
            }),
        });
        edge_id
    }

    pub fn add_binds(
        &mut self,
        scope_id: NodeId,
        binding_id: NodeId,
        owner: SourceOwnership,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let edge_id = structural_edge_id("binds", scope_id, binding_id);
        self.insert_edge(GraphEdge {
            edge_id: edge_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::Binds,
            source_id: scope_id.clone(),
            target_id: Some(binding_id.clone()),
            owner,
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: EdgeFact::Binds(Binds {
                scope_id,
                binding_id,
            }),
        });
        edge_id
    }

    pub fn add_resolves_to(
        &mut self,
        binding_id: NodeId,
        target_id: NodeId,
        owner: SourceOwnership,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_resolves_to_with_resolution(
            binding_id,
            target_id,
            super::schema::Resolution::Exact,
            owner,
            confidence,
            evidence,
        )
    }

    pub fn add_resolves_to_with_resolution(
        &mut self,
        binding_id: NodeId,
        target_id: NodeId,
        resolution: super::schema::Resolution,
        owner: SourceOwnership,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let edge_id = structural_edge_id("resolves-to", binding_id, target_id);
        self.insert_edge(GraphEdge {
            edge_id: edge_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::ResolvesTo,
            source_id: binding_id.clone(),
            target_id: Some(target_id.clone()),
            owner,
            span: None,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: EdgeFact::ResolvesTo(ResolvesTo {
                binding_id,
                target_id,
                resolution,
            }),
        });
        edge_id
    }

    pub fn add_calls(
        &mut self,
        calls: Calls,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let target_id = calls
            .callee_callable_id
            .clone()
            .or_else(|| calls.external_target_id.clone());
        let target_part = match (&target_id, &calls.unresolved_target) {
            (Some(target), _) => IdPart::Id(*target),
            (None, Some(unresolved)) => IdPart::Str(unresolved),
            (None, None) => IdPart::Str("unknown"),
        };
        let edge_id = stable_edge_id(&[
            IdPart::Str("calls"),
            IdPart::Id(calls.caller_callable_id),
            IdPart::Id(calls.call_site_id),
            target_part,
        ]);
        self.insert_edge(GraphEdge {
            edge_id: edge_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::Calls,
            source_id: calls.call_site_id.clone(),
            target_id,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact: EdgeFact::Calls(calls),
        });
        edge_id
    }

    pub fn add_control_flow(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        control_flow: ControlFlow,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "control-flow",
            EdgeKind::ControlFlow,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::ControlFlow(control_flow),
        )
    }

    pub fn add_controls(
        &mut self,
        controls: Controls,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let source_id = controls.condition_id.clone();
        let target_id = controls.controlled_id.clone();
        self.add_semantic_edge(
            "controls",
            EdgeKind::Controls,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::Controls(controls),
        )
    }

    pub fn add_defines(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        defines: Defines,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "defines",
            EdgeKind::Defines,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::Defines(defines),
        )
    }

    pub fn add_uses(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        uses: Uses,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "uses",
            EdgeKind::Uses,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::Uses(uses),
        )
    }

    pub fn add_data_flow(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        data_flow: DataFlow,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "data-flow",
            EdgeKind::DataFlow,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::DataFlow(data_flow),
        )
    }

    pub fn add_parameter_in(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        parameter_in: ParameterIn,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "parameter-in",
            EdgeKind::ParameterIn,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::ParameterIn(parameter_in),
        )
    }

    pub fn add_returns_to(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        returns_to: ReturnsTo,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "returns-to",
            EdgeKind::ReturnsTo,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::ReturnsTo(returns_to),
        )
    }

    pub fn add_parameter_out(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        parameter_out: ParameterOut,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "parameter-out",
            EdgeKind::ParameterOut,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::ParameterOut(parameter_out),
        )
    }

    pub fn add_throws_to(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        throws_to: ThrowsTo,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        self.add_semantic_edge(
            "throws-to",
            EdgeKind::ThrowsTo,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::ThrowsTo(throws_to),
        )
    }

    pub fn add_decomposes_to(
        &mut self,
        decomposes_to: DecomposesTo,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let source_id = decomposes_to.parent_requirement_id.clone();
        let target_id = decomposes_to.child_requirement_id.clone();
        self.add_semantic_edge(
            "decomposes-to",
            EdgeKind::DecomposesTo,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::DecomposesTo(decomposes_to),
        )
    }

    pub fn add_conditions(
        &mut self,
        conditions: Conditions,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let source_id = conditions.condition_requirement_id.clone();
        let target_id = conditions.conditioned_requirement_id.clone();
        self.add_semantic_edge(
            "conditions",
            EdgeKind::Conditions,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::Conditions(conditions),
        )
    }

    pub fn add_orders(
        &mut self,
        orders: Orders,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let source_id = orders.predecessor_requirement_id.clone();
        let target_id = orders.successor_requirement_id.clone();
        self.add_semantic_edge(
            "orders",
            EdgeKind::Orders,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::Orders(orders),
        )
    }

    pub fn add_traces_to(
        &mut self,
        traces_to: TracesTo,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let source_id = traces_to.requirement_id.clone();
        let target_id = traces_to.code_fact_id.clone();
        self.add_semantic_edge(
            "traces-to",
            EdgeKind::TracesTo,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::TracesTo(traces_to),
        )
    }

    pub fn add_depends_on_domain_knowledge(
        &mut self,
        depends_on_domain_knowledge: DependsOnDomainKnowledge,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> EdgeId {
        let source_id = depends_on_domain_knowledge.requirement_id.clone();
        let target_id = depends_on_domain_knowledge.domain_knowledge_id.clone();
        self.add_semantic_edge(
            "depends-on-domain-knowledge",
            EdgeKind::DependsOnDomainKnowledge,
            source_id,
            target_id,
            owner,
            span,
            confidence,
            evidence,
            EdgeFact::DependsOnDomainKnowledge(depends_on_domain_knowledge),
        )
    }

    fn add_structural_node(
        &mut self,
        node_id: NodeId,
        kind: NodeKind,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
        fact: NodeFact,
    ) -> NodeId {
        self.insert_node(GraphNode {
            node_id: node_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact,
        });
        node_id
    }

    fn add_semantic_edge(
        &mut self,
        family: &str,
        kind: EdgeKind,
        source_id: NodeId,
        target_id: NodeId,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
        fact: EdgeFact,
    ) -> EdgeId {
        let edge_id = structural_edge_id(family, source_id, target_id);
        self.insert_edge(GraphEdge {
            edge_id: edge_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind,
            source_id,
            target_id: Some(target_id),
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact,
        });
        edge_id
    }

    pub fn finish(self) -> ProgramSupergraph {
        let mut graph = ProgramSupergraph {
            schema_version: SCHEMA_VERSION.to_string(),
            language: self.language,
            root: self.root,
            nodes: self.nodes.into_values().collect(),
            edges: self.edges.into_values().collect(),
            indexes: GraphIndexes::default(),
            id_cache: Default::default(),
        };
        refresh_uncertainty(&mut graph);
        refresh_provenance(&mut graph);
        refresh_fact_identity(&mut graph);
        sort_graph(&mut graph);
        graph.indexes = build_indexes(&graph.nodes, &graph.edges);
        graph
    }
}

pub fn rehydrate_program_supergraph(mut graph: ProgramSupergraph) -> ProgramSupergraph {
    refresh_uncertainty(&mut graph);
    refresh_provenance(&mut graph);
    refresh_fact_identity(&mut graph);
    sort_graph(&mut graph);
    graph.indexes = build_indexes(&graph.nodes, &graph.edges);
    graph
}

pub fn stable_id(tag: Tag, parts: &[IdPart<'_>]) -> NodeId {
    NodeId::stable(tag, parts)
}

pub fn stable_edge_id(parts: &[IdPart<'_>]) -> EdgeId {
    EdgeId::stable(parts)
}

struct StableHashWriter {
    hash: u64,
}

impl StableHashWriter {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325_u64;

    fn new() -> Self {
        Self {
            hash: Self::OFFSET_BASIS,
        }
    }

    fn reset(&mut self) {
        self.hash = Self::OFFSET_BASIS;
    }

    fn finish_part(&mut self) {
        self.hash ^= 0xff;
        self.hash = self.hash.wrapping_mul(0x100000001b3);
    }

    fn finish(&mut self) -> PayloadHash {
        self.finish_part();
        PayloadHash(NonZeroU64::new(self.hash).unwrap_or(NonZeroU64::MIN))
    }
}

impl Write for StableHashWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for byte in buf {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(0x100000001b3);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn stable_payload_hash_with_writer(
    payload: &impl Serialize,
    writer: &mut StableHashWriter,
) -> PayloadHash {
    writer.reset();
    with_legacy_id_text(|| {
        let mut serializer = serde_json::Serializer::new(&mut *writer);
        payload
            .serialize(&mut serializer)
            .expect("identity payload serialization should not fail");
    });
    writer.finish()
}

pub fn artifact_id(path: &str) -> NodeId {
    stable_id(Tag::Artifact, id_parts![path])
}

pub fn scope_id(artifact_id: NodeId, parts: &[&str]) -> NodeId {
    let mut id_parts = Vec::with_capacity(parts.len() + 1);
    id_parts.push(IdPart::Id(artifact_id));
    id_parts.extend(parts.iter().map(|part| IdPart::Str(part)));
    stable_id(Tag::Scope, &id_parts)
}

pub fn binding_id(scope_id: NodeId, name: &str, span: SourceSpan) -> NodeId {
    stable_id(Tag::Binding, id_parts![scope_id, name, &span_key(span)])
}

pub fn callable_id(qualified_name: &str) -> NodeId {
    callable_id_from_text(qualified_name)
}

pub fn call_site_id(
    artifact_id: NodeId,
    enclosing_callable_id: NodeId,
    callee_expression: &str,
    span: SourceSpan,
) -> NodeId {
    stable_id(
        Tag::CallSite,
        id_parts![
            artifact_id,
            enclosing_callable_id,
            callee_expression,
            &span_key(span),
        ],
    )
}

pub fn statement_id(callable_id: NodeId, kind: &str, span: SourceSpan, ordinal: usize) -> NodeId {
    stable_id(
        Tag::Statement,
        id_parts![callable_id, kind, &span_key(span), &ordinal.to_string()],
    )
}

pub fn expression_id(callable_id: NodeId, kind: &str, span: SourceSpan, ordinal: usize) -> NodeId {
    stable_id(
        Tag::Expression,
        id_parts![callable_id, kind, &span_key(span), &ordinal.to_string()],
    )
}

pub fn condition_id(callable_id: NodeId, kind: &str, span: SourceSpan, ordinal: usize) -> NodeId {
    stable_id(
        Tag::Condition,
        id_parts![callable_id, kind, &span_key(span), &ordinal.to_string()],
    )
}

pub fn symbol_id(scope_id: NodeId, name: &str, span: Option<SourceSpan>) -> NodeId {
    stable_id(
        Tag::Symbol,
        id_parts![scope_id, name, &span.map(span_key).unwrap_or_default()],
    )
}

pub fn definition_id(callable_id: NodeId, name: &str, span: SourceSpan) -> NodeId {
    stable_id(Tag::Definition, id_parts![callable_id, name, &span_key(span)])
}

pub fn use_id(callable_id: NodeId, name: &str, span: SourceSpan) -> NodeId {
    stable_id(Tag::Use, id_parts![callable_id, name, &span_key(span)])
}

pub fn value_id(callable_id: Option<NodeId>, kind: &str, span: Option<SourceSpan>) -> NodeId {
    let callable_part = callable_id.map_or(IdPart::Str(""), IdPart::Id);
    stable_id(
        Tag::Value,
        &[
            callable_part,
            IdPart::Str(kind),
            IdPart::Str(&span.map(span_key).unwrap_or_default()),
        ],
    )
}

pub fn basic_block_id(callable_id: NodeId, ordinal: usize) -> NodeId {
    stable_id(Tag::BasicBlock, id_parts![callable_id, &ordinal.to_string()])
}

pub fn domain_knowledge_id(scope_key: &str, summary: &str) -> NodeId {
    stable_id(Tag::DomainKnowledge, id_parts![scope_key, summary])
}

pub fn external_target_id(
    ecosystem: &str,
    qualified_name: &str,
    member_path: Option<&str>,
) -> NodeId {
    stable_id(
        Tag::ExternalTarget,
        id_parts![ecosystem, qualified_name, member_path.unwrap_or_default()],
    )
}

pub fn diagnostic_id(message: &str, span: Option<SourceSpan>) -> NodeId {
    stable_id(
        Tag::Diagnostic,
        id_parts![message, &span.map(span_key).unwrap_or_default()],
    )
}

pub fn source_owner(
    artifact_id: impl Into<Option<NodeId>>,
    scope_id: impl Into<Option<NodeId>>,
    callable_id: impl Into<Option<NodeId>>,
) -> SourceOwnership {
    SourceOwnership {
        artifact_id: artifact_id.into(),
        scope_id: scope_id.into(),
        callable_id: callable_id.into(),
    }
}

/// Runs `work` over contiguous chunks of `items` on scoped threads. Chunks are disjoint, so
/// the result is identical to a sequential pass when `work` only touches its own items.
pub(crate) fn par_chunks_mut<T: Send>(items: &mut [T], work: impl Fn(&mut [T]) + Sync) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk_size = items.len().div_ceil(threads).max(2048);
    if items.len() <= chunk_size {
        work(items);
        return;
    }
    let work = &work;
    std::thread::scope(|scope| {
        for chunk in items.chunks_mut(chunk_size) {
            scope.spawn(move || work(chunk));
        }
    });
}

pub fn sort_graph(graph: &mut ProgramSupergraph) {
    par_chunks_mut(&mut graph.nodes, |nodes| {
        for node in nodes {
            sort_evidence(&mut node.evidence);
        }
    });
    par_chunks_mut(&mut graph.edges, |edges| {
        for edge in edges {
            sort_evidence(&mut edge.evidence);
        }
    });
    let ProgramSupergraph { nodes, edges, .. } = graph;
    std::thread::scope(|scope| {
        scope.spawn(|| nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id)));
        edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    });
}

#[derive(Default)]
struct IndexScratch {
    node_position_by_id: IdMap<NodeId, u32>,
    edge_position_by_id: IdMap<EdgeId, u32>,
    nodes_by_kind: BTreeMap<NodeKind, Vec<NodeId>>,
    edges_by_kind: BTreeMap<EdgeKind, Vec<EdgeId>>,
    nodes_by_uncertainty: BTreeMap<Uncertainty, Vec<NodeId>>,
    edges_by_uncertainty: BTreeMap<Uncertainty, Vec<EdgeId>>,
    outgoing_edges_by_node: BTreeMap<NodeId, Vec<EdgeId>>,
    incoming_edges_by_node: BTreeMap<NodeId, Vec<EdgeId>>,
    source_span_to_nodes: BTreeMap<SourceSpanIndexKey, Vec<NodeId>>,
    artifact_to_nodes: BTreeMap<NodeId, Vec<NodeId>>,
    callable_to_nodes: BTreeMap<NodeId, Vec<NodeId>>,
    owner_to_nodes: BTreeMap<NodeId, Vec<NodeId>>,
    owner_to_edges: BTreeMap<NodeId, Vec<EdgeId>>,
    symbol_to_definitions: BTreeMap<NodeId, Vec<NodeId>>,
    symbol_to_uses: BTreeMap<NodeId, Vec<NodeId>>,
    requirement_to_code: BTreeMap<NodeId, Vec<NodeId>>,
    code_to_requirements: BTreeMap<NodeId, Vec<NodeId>>,
    requirement_to_domain_knowledge: BTreeMap<NodeId, Vec<NodeId>>,
    domain_knowledge_to_requirements: BTreeMap<NodeId, Vec<NodeId>>,
    calls_by_caller: BTreeMap<NodeId, Vec<EdgeId>>,
    calls_by_concrete_target: BTreeMap<NodeId, Vec<EdgeId>>,
    call_site_to_calls: BTreeMap<NodeId, Vec<EdgeId>>,
    caller_to_concrete_target_calls: BTreeMap<NodeId, BTreeMap<NodeId, Vec<EdgeId>>>,
    caller_to_concrete_call_targets: BTreeMap<NodeId, Vec<NodeId>>,
}

impl IndexScratch {
    fn finish(self) -> GraphIndexes {
        GraphIndexes {
            node_position_by_id: self.node_position_by_id,
            edge_position_by_id: self.edge_position_by_id,
            nodes_by_kind: MultiMap::from_btree(self.nodes_by_kind),
            edges_by_kind: MultiMap::from_btree(self.edges_by_kind),
            nodes_by_uncertainty: MultiMap::from_btree(self.nodes_by_uncertainty),
            edges_by_uncertainty: MultiMap::from_btree(self.edges_by_uncertainty),
            outgoing_edges_by_node: MultiMap::from_btree(self.outgoing_edges_by_node),
            incoming_edges_by_node: MultiMap::from_btree(self.incoming_edges_by_node),
            source_span_to_nodes: MultiMap::from_btree(self.source_span_to_nodes),
            artifact_to_nodes: MultiMap::from_btree(self.artifact_to_nodes),
            callable_to_nodes: MultiMap::from_btree(self.callable_to_nodes),
            owner_to_nodes: MultiMap::from_btree(self.owner_to_nodes),
            owner_to_edges: MultiMap::from_btree(self.owner_to_edges),
            symbol_to_definitions: MultiMap::from_btree(self.symbol_to_definitions),
            symbol_to_uses: MultiMap::from_btree(self.symbol_to_uses),
            requirement_to_code: MultiMap::from_btree(self.requirement_to_code),
            code_to_requirements: MultiMap::from_btree(self.code_to_requirements),
            requirement_to_domain_knowledge: MultiMap::from_btree(self.requirement_to_domain_knowledge),
            domain_knowledge_to_requirements: MultiMap::from_btree(self.domain_knowledge_to_requirements),
            calls_by_caller: MultiMap::from_btree(self.calls_by_caller),
            calls_by_concrete_target: MultiMap::from_btree(self.calls_by_concrete_target),
            call_site_to_calls: MultiMap::from_btree(self.call_site_to_calls),
            caller_to_concrete_target_calls: self.caller_to_concrete_target_calls,
            caller_to_concrete_call_targets: MultiMap::from_btree(self.caller_to_concrete_call_targets),
        }
    }
}

pub fn build_indexes(nodes: &[GraphNode], edges: &[GraphEdge]) -> GraphIndexes {
    // Each group fills a disjoint set of maps, so the groups can be built concurrently.
    let (node_a, node_b, node_c, node_d, edge_p, edge_a, edge_o, edge_i, edge_c) = std::thread::scope(|scope| {
        let node_a = scope.spawn(|| build_node_indexes(nodes, 0));
        let node_b = scope.spawn(|| build_node_indexes(nodes, 1));
        let node_c = scope.spawn(|| build_node_indexes(nodes, 2));
        let node_d = scope.spawn(|| build_node_indexes(nodes, 3));
        let edge_p = scope.spawn(|| build_edge_indexes(edges, 0));
        let edge_a = scope.spawn(|| build_edge_indexes(edges, 1));
        let edge_o = scope.spawn(|| build_edge_indexes(edges, 2));
        let edge_i = scope.spawn(|| build_edge_indexes(edges, 3));
        let edge_c = build_edge_indexes(edges, 4);
        (
            node_a.join().expect("node index thread"),
            node_b.join().expect("node index thread"),
            node_c.join().expect("node index thread"),
            node_d.join().expect("node index thread"),
            edge_p.join().expect("edge index thread"),
            edge_a.join().expect("edge index thread"),
            edge_o.join().expect("edge index thread"),
            edge_i.join().expect("edge index thread"),
            edge_c,
        )
    });
    GraphIndexes {
        node_position_by_id: node_a.node_position_by_id,
        nodes_by_kind: node_a.nodes_by_kind,
        nodes_by_uncertainty: node_a.nodes_by_uncertainty,
        source_span_to_nodes: node_b.source_span_to_nodes,
        artifact_to_nodes: node_c.artifact_to_nodes,
        callable_to_nodes: node_c.callable_to_nodes,
        owner_to_nodes: node_c.owner_to_nodes,
        symbol_to_definitions: node_d.symbol_to_definitions,
        symbol_to_uses: node_d.symbol_to_uses,
        edge_position_by_id: edge_p.edge_position_by_id,
        edges_by_kind: edge_a.edges_by_kind,
        edges_by_uncertainty: edge_a.edges_by_uncertainty,
        owner_to_edges: edge_a.owner_to_edges,
        outgoing_edges_by_node: edge_o.outgoing_edges_by_node,
        incoming_edges_by_node: edge_i.incoming_edges_by_node,
        requirement_to_code: edge_c.requirement_to_code,
        code_to_requirements: edge_c.code_to_requirements,
        requirement_to_domain_knowledge: edge_c.requirement_to_domain_knowledge,
        domain_knowledge_to_requirements: edge_c.domain_knowledge_to_requirements,
        calls_by_caller: edge_c.calls_by_caller,
        calls_by_concrete_target: edge_c.calls_by_concrete_target,
        call_site_to_calls: edge_c.call_site_to_calls,
        caller_to_concrete_target_calls: edge_c.caller_to_concrete_target_calls,
        caller_to_concrete_call_targets: edge_c.caller_to_concrete_call_targets,
    }
}

/// Builds the node-derived maps of one group (0..=3); every other map stays empty.
fn build_node_indexes(nodes: &[GraphNode], group: u8) -> GraphIndexes {
    let mut indexes = IndexScratch::default();
    for (position, node) in nodes.iter().enumerate() {
        match group {
            0 => {
                indexes
                    .node_position_by_id
                    .insert(node.node_id.clone(), position as u32);
                indexes
                    .nodes_by_kind
                    .entry(node.kind)
                    .or_default()
                    .push(node.node_id.clone());
                indexes
                    .nodes_by_uncertainty
                    .entry(node.uncertainty)
                    .or_default()
                    .push(node.node_id.clone());
            }
            1 => {
                if let Some(span) = node.span {
                    indexes
                        .source_span_to_nodes
                        .entry(SourceSpanIndexKey {
                            artifact_id: node.owner.artifact_id.clone(),
                            span,
                        })
                        .or_default()
                        .push(node.node_id.clone());
                }
            }
            2 => {
                if let Some(artifact_id) = &node.owner.artifact_id {
                    indexes
                        .artifact_to_nodes
                        .entry(artifact_id.clone())
                        .or_default()
                        .push(node.node_id.clone());
                }
                if let Some(callable_id) = &node.owner.callable_id {
                    indexes
                        .callable_to_nodes
                        .entry(callable_id.clone())
                        .or_default()
                        .push(node.node_id.clone());
                }
                push_owner_node(&mut indexes.owner_to_nodes, &node.owner, &node.node_id);
            }
            _ => match &node.fact {
                NodeFact::Definition(definition) => {
                    if let Some(symbol_id) = &definition.symbol_id {
                        indexes
                            .symbol_to_definitions
                            .entry(symbol_id.clone())
                            .or_default()
                            .push(node.node_id.clone());
                    }
                }
                NodeFact::Use(use_fact) => {
                    if let Some(symbol_id) = &use_fact.symbol_id {
                        indexes
                            .symbol_to_uses
                            .entry(symbol_id.clone())
                            .or_default()
                            .push(node.node_id.clone());
                    }
                }
                _ => {}
            },
        }
    }
    dedup_indexes(&mut indexes);
    indexes.finish()
}

/// Builds the edge-derived maps of one group (0..=4); every other map stays empty.
fn build_edge_indexes(edges: &[GraphEdge], group: u8) -> GraphIndexes {
    let mut indexes = IndexScratch::default();
    for (position, edge) in edges.iter().enumerate() {
        match group {
            0 => {
                indexes
                    .edge_position_by_id
                    .insert(edge.edge_id.clone(), position as u32);
            }
            1 => {
                indexes
                    .edges_by_kind
                    .entry(edge.kind)
                    .or_default()
                    .push(edge.edge_id.clone());
                indexes
                    .edges_by_uncertainty
                    .entry(edge.uncertainty)
                    .or_default()
                    .push(edge.edge_id.clone());
                push_owner_edge(&mut indexes.owner_to_edges, &edge.owner, &edge.edge_id);
            }
            2 => {
                indexes
                    .outgoing_edges_by_node
                    .entry(edge.source_id.clone())
                    .or_default()
                    .push(edge.edge_id.clone());
            }
            3 => {
                if let Some(target_id) = &edge.target_id {
                    indexes
                        .incoming_edges_by_node
                        .entry(target_id.clone())
                        .or_default()
                        .push(edge.edge_id.clone());
                }
            }
            _ => {
                if let EdgeFact::TracesTo(trace) = &edge.fact {
                    indexes
                        .requirement_to_code
                        .entry(trace.requirement_id.clone())
                        .or_default()
                        .push(trace.code_fact_id.clone());
                    indexes
                        .code_to_requirements
                        .entry(trace.code_fact_id.clone())
                        .or_default()
                        .push(trace.requirement_id.clone());
                }
                if let EdgeFact::DependsOnDomainKnowledge(dependency) = &edge.fact {
                    indexes
                        .requirement_to_domain_knowledge
                        .entry(dependency.requirement_id.clone())
                        .or_default()
                        .push(dependency.domain_knowledge_id.clone());
                    indexes
                        .domain_knowledge_to_requirements
                        .entry(dependency.domain_knowledge_id.clone())
                        .or_default()
                        .push(dependency.requirement_id.clone());
                }
                if let EdgeFact::Calls(calls) = &edge.fact {
                    indexes
                        .calls_by_caller
                        .entry(calls.caller_callable_id.clone())
                        .or_default()
                        .push(edge.edge_id.clone());
                    if let Some(target_id) = edge.target_id.as_ref() {
                        indexes
                            .calls_by_concrete_target
                            .entry(target_id.clone())
                            .or_default()
                            .push(edge.edge_id.clone());
                    }
                    indexes
                        .call_site_to_calls
                        .entry(calls.call_site_id.clone())
                        .or_default()
                        .push(edge.edge_id.clone());
                    if let Some(target_id) = edge.target_id.as_ref() {
                        indexes
                            .caller_to_concrete_target_calls
                            .entry(calls.caller_callable_id.clone())
                            .or_default()
                            .entry(target_id.clone())
                            .or_default()
                            .push(edge.edge_id.clone());
                        indexes
                            .caller_to_concrete_call_targets
                            .entry(calls.caller_callable_id.clone())
                            .or_default()
                            .push(target_id.clone());
                    }
                }
            }
        }
    }
    dedup_indexes(&mut indexes);
    indexes.finish()
}

fn dedup_indexes(indexes: &mut IndexScratch) {
    dedup_index(&mut indexes.nodes_by_kind);
    dedup_index(&mut indexes.edges_by_kind);
    dedup_index(&mut indexes.nodes_by_uncertainty);
    dedup_index(&mut indexes.edges_by_uncertainty);
    dedup_index(&mut indexes.outgoing_edges_by_node);
    dedup_index(&mut indexes.incoming_edges_by_node);
    dedup_index(&mut indexes.source_span_to_nodes);
    dedup_index(&mut indexes.artifact_to_nodes);
    dedup_index(&mut indexes.callable_to_nodes);
    dedup_index(&mut indexes.owner_to_nodes);
    dedup_index(&mut indexes.owner_to_edges);
    dedup_index(&mut indexes.symbol_to_definitions);
    dedup_index(&mut indexes.symbol_to_uses);
    dedup_index(&mut indexes.requirement_to_code);
    dedup_index(&mut indexes.code_to_requirements);
    dedup_index(&mut indexes.requirement_to_domain_knowledge);
    dedup_index(&mut indexes.domain_knowledge_to_requirements);
    dedup_index(&mut indexes.calls_by_caller);
    dedup_index(&mut indexes.calls_by_concrete_target);
    dedup_index(&mut indexes.call_site_to_calls);
    dedup_nested_index(&mut indexes.caller_to_concrete_target_calls);
    dedup_index(&mut indexes.caller_to_concrete_call_targets);
}

fn structural_edge_id(kind: &str, source_id: NodeId, target_id: NodeId) -> EdgeId {
    stable_edge_id(id_parts![kind, source_id, target_id])
}

fn span_key(span: SourceSpan) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        span.start_byte,
        span.end_byte,
        span.start_row,
        span.start_column,
        span.end_row,
        span.end_column
    )
}

fn sort_evidence(evidence: &mut Vec<Evidence>) {
    evidence.sort();
    evidence.dedup();
}

pub fn refresh_provenance(graph: &mut ProgramSupergraph) {
    let artifacts = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Artifact(artifact) => {
                Some((artifact.artifact_id.clone(), artifact.content_hash.clone()))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();

    par_chunks_mut(&mut graph.nodes, |nodes| {
        for node in nodes {
            enrich_evidence(&mut node.evidence, &node.owner, node.span, &artifacts);
        }
    });
    par_chunks_mut(&mut graph.edges, |edges| {
        for edge in edges {
            enrich_evidence(&mut edge.evidence, &edge.owner, edge.span, &artifacts);
        }
    });
}

pub fn refresh_fact_identity(graph: &mut ProgramSupergraph) {
    par_chunks_mut(&mut graph.nodes, |nodes| {
        let mut payload_writer = StableHashWriter::new();
        for node in nodes {
            sort_evidence(&mut node.evidence);
            node.payload_hash = Some(stable_payload_hash_with_writer(
                &NodeIdentityPayload { node },
                &mut payload_writer,
            ));
            node.fact_id = Some(FactId::stable(id_parts![node.node_id, node.payload_hash.expect("payload hash")]));
        }
    });
    par_chunks_mut(&mut graph.edges, |edges| {
        let mut payload_writer = StableHashWriter::new();
        for edge in edges {
            sort_evidence(&mut edge.evidence);
            edge.payload_hash = Some(stable_payload_hash_with_writer(
                &EdgeIdentityPayload { edge },
                &mut payload_writer,
            ));
            edge.fact_id = Some(FactId::stable(id_parts![edge.edge_id, edge.payload_hash.expect("payload hash")]));
        }
    });
}

struct NodeIdentityPayload<'a> {
    node: &'a GraphNode,
}

impl Serialize for NodeIdentityPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Preserve serde_json::json! object key order, which is sorted by key.
        let mut map = serializer.serialize_map(Some(7))?;
        map.serialize_entry("confidence", &self.node.confidence)?;
        map.serialize_entry("evidence", &self.node.evidence)?;
        map.serialize_entry("fact", &self.node.fact)?;
        map.serialize_entry("kind", &self.node.kind)?;
        map.serialize_entry("owner", &self.node.owner)?;
        map.serialize_entry("span", &self.node.span)?;
        map.serialize_entry("uncertainty", &self.node.uncertainty)?;
        map.end()
    }
}

struct EdgeIdentityPayload<'a> {
    edge: &'a GraphEdge,
}

impl Serialize for EdgeIdentityPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Preserve serde_json::json! object key order, which is sorted by key.
        let mut map = serializer.serialize_map(Some(9))?;
        map.serialize_entry("confidence", &self.edge.confidence)?;
        map.serialize_entry("evidence", &self.edge.evidence)?;
        map.serialize_entry("fact", &self.edge.fact)?;
        map.serialize_entry("kind", &self.edge.kind)?;
        map.serialize_entry("owner", &self.edge.owner)?;
        map.serialize_entry("source_id", &self.edge.source_id)?;
        map.serialize_entry("span", &self.edge.span)?;
        map.serialize_entry("target_id", &self.edge.target_id)?;
        map.serialize_entry("uncertainty", &self.edge.uncertainty)?;
        map.end()
    }
}

pub fn refresh_uncertainty(graph: &mut ProgramSupergraph) {
    for node in &mut graph.nodes {
        node.uncertainty = node_uncertainty(&node.confidence, &node.fact);
    }
    for edge in &mut graph.edges {
        edge.uncertainty = edge_uncertainty(&edge.confidence, &edge.fact);
    }
}

pub fn node_uncertainty(confidence: &Confidence, fact: &NodeFact) -> Uncertainty {
    match fact {
        NodeFact::Symbol(symbol) => uncertainty_from_resolution(symbol.resolution),
        NodeFact::Binding(binding) => match &binding.target {
            super::schema::BindingTarget::External(_) => Uncertainty::External,
            super::schema::BindingTarget::Unresolved(_) => Uncertainty::Unresolved,
            _ => uncertainty_from_confidence(confidence),
        },
        NodeFact::ExternalTarget(_) => Uncertainty::External,
        NodeFact::DomainKnowledge(domain) => {
            uncertainty_from_domain_knowledge_status(domain.status)
        }
        NodeFact::Diagnostic(diagnostic) => uncertainty_from_diagnostic_kind(diagnostic.kind),
        _ => uncertainty_from_confidence(confidence),
    }
}

pub fn edge_uncertainty(confidence: &Confidence, fact: &EdgeFact) -> Uncertainty {
    match fact {
        EdgeFact::ResolvesTo(resolution) => uncertainty_from_resolution(resolution.resolution),
        EdgeFact::Calls(calls) => uncertainty_from_resolution(calls.resolution),
        EdgeFact::ControlFlow(edge) if edge.precision.contains("approximation") => {
            Uncertainty::Probable
        }
        EdgeFact::Controls(edge) if edge.precision.contains("approximation") => {
            Uncertainty::Probable
        }
        EdgeFact::DataFlow(edge) if edge.precision.contains("alias") => Uncertainty::Possible,
        EdgeFact::ParameterIn(edge) if edge.precision.contains("possible-call-target") => {
            Uncertainty::Possible
        }
        EdgeFact::ParameterIn(edge) if edge.precision.contains("unavailable") => {
            Uncertainty::Possible
        }
        EdgeFact::ParameterIn(edge) if edge.precision.contains("without-recoverable") => {
            Uncertainty::Unresolved
        }
        EdgeFact::ParameterOut(edge)
            if edge.precision.contains("alias") || edge.precision.contains("possible") =>
        {
            Uncertainty::Possible
        }
        EdgeFact::ReturnsTo(edge) if edge.precision.contains("possible") => Uncertainty::Possible,
        EdgeFact::ThrowsTo(edge) if edge.precision.contains("possible") => Uncertainty::Possible,
        EdgeFact::ThrowsTo(edge)
            if edge.precision.starts_with("sg084") && *confidence == Confidence::Probable =>
        {
            Uncertainty::Possible
        }
        EdgeFact::ThrowsTo(edge) if edge.precision.contains("without-exception-type-filtering") => {
            Uncertainty::Probable
        }
        EdgeFact::TracesTo(edge) if edge.precision.contains("ambiguous") => Uncertainty::Ambiguous,
        EdgeFact::DependsOnDomainKnowledge(edge) if edge.precision.contains("stale") => {
            Uncertainty::Stale
        }
        EdgeFact::DependsOnDomainKnowledge(edge) if edge.precision.contains("missing") => {
            Uncertainty::Unresolved
        }
        EdgeFact::DependsOnDomainKnowledge(edge) if edge.precision.contains("possible") => {
            Uncertainty::Possible
        }
        EdgeFact::DependsOnDomainKnowledge(_) => Uncertainty::Probable,
        _ => uncertainty_from_confidence(confidence),
    }
}

fn enrich_evidence(
    evidence: &mut [Evidence],
    owner: &SourceOwnership,
    span: Option<SourceSpan>,
    artifacts: &BTreeMap<NodeId, Option<Sym>>,
) {
    for evidence in evidence {
        if evidence.source_id.is_none() {
            evidence.source_id = owner.artifact_id.clone();
        }
        if evidence.source_span.is_none() {
            evidence.source_span = span;
        }
        let Some(source_id) = evidence.source_id.as_ref() else {
            continue;
        };
        let Some(content_hash) = artifacts.get(source_id) else {
            continue;
        };
        if evidence.content_hash.is_none() {
            evidence.content_hash = *content_hash;
        }
    }
}

fn push_owner_node(
    index: &mut BTreeMap<NodeId, Vec<NodeId>>,
    owner: &SourceOwnership,
    node_id: &NodeId,
) {
    if let Some(artifact_id) = &owner.artifact_id {
        index
            .entry(artifact_id.clone())
            .or_default()
            .push(node_id.clone());
    }
    if let Some(scope_id) = &owner.scope_id {
        index
            .entry(scope_id.clone())
            .or_default()
            .push(node_id.clone());
    }
    if let Some(callable_id) = &owner.callable_id {
        index
            .entry(callable_id.clone())
            .or_default()
            .push(node_id.clone());
    }
}

fn push_owner_edge(
    index: &mut BTreeMap<NodeId, Vec<EdgeId>>,
    owner: &SourceOwnership,
    edge_id: &EdgeId,
) {
    if let Some(artifact_id) = &owner.artifact_id {
        index
            .entry(artifact_id.clone())
            .or_default()
            .push(edge_id.clone());
    }
    if let Some(scope_id) = &owner.scope_id {
        index
            .entry(scope_id.clone())
            .or_default()
            .push(edge_id.clone());
    }
    if let Some(callable_id) = &owner.callable_id {
        index
            .entry(callable_id.clone())
            .or_default()
            .push(edge_id.clone());
    }
}

fn dedup_index<K: Ord, V: Ord>(index: &mut BTreeMap<K, Vec<V>>) {
    for values in index.values_mut() {
        values.sort();
        values.dedup();
    }
}

fn dedup_nested_index<K: Ord, L: Ord, V: Ord>(index: &mut BTreeMap<K, BTreeMap<L, Vec<V>>>) {
    for nested in index.values_mut() {
        dedup_index(nested);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supergraph::ids::test_support::test_id;
    use crate::supergraph::schema::{
        ArgumentShape, BasicBlockKind, BindingKind, BindingTarget, CallEdgeKind, CallableKind,
        ConditionKind, Conditions, ControlFlow, ControlFlowKind, ControlFlowNode,
        ControlFlowNodeRole, Controls, DataFlow, DataFlowKind, DataFlowNode, DataFlowNodeRole,
        DecomposesTo, Defines, DefinitionKind, DependsOnDomainKnowledge, DiagnosticKind,
        DispatchKind, DomainKnowledgeScope, DomainKnowledgeSource, DomainKnowledgeStatus,
        EvidenceKind, ExpressionKind, ExternalTargetKind, FallthroughBehavior,
        NormalizedExpression, Orders, ParameterIn, ParameterOut, Requirement, RequirementKind,
        Resolution, ReturnsTo, Scope, ScopeBindingBehavior, ScopeKind, ScopeVariant, Severity,
        Signature, StatementKind, SymbolKind, ThrowsTo, ThrowsToTargetKind, TracesTo, Uncertainty,
        UseKind, Uses, ValueKind, ValueLiteral, ValueRole,
    };
    use crate::supergraph::views::StructuralGraphView;

    #[test]
    fn structural_node_families_are_constructible_with_envelope_metadata() {
        let artifact_id = test_id("artifact:main");
        let scope_id = test_id("scope:module");
        let callable_id = test_id("callable:main.process");
        let owner = source_owner(
            Some(artifact_id.clone()),
            Some(scope_id.clone()),
            Some(callable_id.clone()),
        );
        let span = span(10, 20);
        let evidence = vec![Evidence {
            kind: EvidenceKind::Parser,
            summary: Sym::new("normalized structural fact"),
            source_id: Some(artifact_id.clone()),
            source_span: Some(span),
            content_hash: Some(Sym::new("sha256:test-source")),
            syntax: Some(SyntaxReference {
                kind: Sym::new("integer"),
                key_prefix: Some(Sym::new("expr")),
                field_path: Box::new([Sym::new("value")]),
            }),
        }];

        let statement_id = statement_id(callable_id, "assignment", span, 0);
        let expression_id = expression_id(callable_id, "literal", span, 0);
        let condition_id = condition_id(callable_id, "branch", span, 0);
        let symbol_id = symbol_id(scope_id, "batch_size", Some(span));
        let definition_id = definition_id(callable_id, "batch_size", span);
        let use_id = use_id(callable_id, "batch_size", span);
        let value_id = value_id(Some(callable_id), "literal", Some(span));
        let basic_block_id = basic_block_id(callable_id, 0);
        let domain_knowledge_id = domain_knowledge_id("repository", "batch sizes are user visible");

        let mut builder = ProgramSupergraphBuilder::new("repo", "python");
        builder.add_statement(
            Statement {
                statement_id: statement_id.clone(),
                callable_id: callable_id.clone(),
                parent_statement_id: None,
                kind: StatementKind::Assignment,
                ordinal: 0,
                child_statement_ids: Vec::new(),
                expression_ids: vec![expression_id.clone()],
                control_effects: Vec::new(),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_expression(
            Expression {
                expression_id: expression_id.clone(),
                callable_id: callable_id.clone(),
                statement_id: Some(statement_id.clone()),
                parent_expression_id: None,
                kind: ExpressionKind::Literal,
                ordinal: 0,
                child_expression_ids: Vec::new(),
                symbol_id: None,
                value_id: Some(value_id.clone()),
                original_text: Some(Sym::new("25")),
                normalized: NormalizedExpression {
                    canonical: Some(Sym::new("25")),
                    literal: Some(ValueLiteral::Integer("25".to_string())),
                    ..Default::default()
                },
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_condition(
            Condition {
                condition_id: condition_id.clone(),
                callable_id: callable_id.clone(),
                statement_id: Some(statement_id.clone()),
                expression_id: Some(expression_id.clone()),
                kind: ConditionKind::Branch,
                controlled_statement_ids: vec![statement_id.clone()],
                outcome_labels: vec![Sym::new("true"), Sym::from("false".to_string())],
                regions: Vec::new(),
                continuation: None,
                fallthrough: FallthroughBehavior::Unknown,
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_symbol(
            Symbol {
                symbol_id: symbol_id.clone(),
                scope_id: scope_id.clone(),
                name: Sym::new("batch_size"),
                kind: SymbolKind::Local,
                binding_id: None,
                resolution: Resolution::Exact,
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_definition(
            Definition {
                definition_id: definition_id.clone(),
                callable_id: callable_id.clone(),
                symbol_id: Some(symbol_id.clone()),
                value_id: Some(value_id.clone()),
                kind: DefinitionKind::Assignment,
                name: Some(Sym::new("batch_size")),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_use(
            Use {
                use_id: use_id.clone(),
                callable_id: callable_id.clone(),
                symbol_id: Some(symbol_id.clone()),
                value_id: Some(value_id.clone()),
                kind: UseKind::Read,
                name: Some(Sym::new("batch_size")),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_value(
            Value {
                value_id: value_id.clone(),
                callable_id: Some(callable_id.clone()),
                kind: ValueKind::Literal,
                role: ValueRole::Unknown,
                symbol_id: Some(symbol_id.clone()),
                expression_id: Some(expression_id.clone()),
                call_site_id: None,
                name: None,
                ordinal: None,
                state_of_value_id: None,
                type_hint: Some(Sym::new("int")),
                literal: Some(ValueLiteral::Integer("25".to_string())),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_basic_block(
            BasicBlock {
                basic_block_id: basic_block_id.clone(),
                callable_id: callable_id.clone(),
                kind: BasicBlockKind::StraightLine,
                ordinal: 0,
                statement_ids: vec![statement_id.clone()],
                entry_node_id: Some(statement_id.clone()),
                exit_node_id: Some(statement_id.clone()),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            evidence.clone(),
        );
        builder.add_domain_knowledge(
            DomainKnowledge {
                domain_knowledge_id: domain_knowledge_id.clone(),
                scope: DomainKnowledgeScope::Repository,
                summary: "batch sizes are user visible".to_string(),
                source: DomainKnowledgeSource::User,
                applies_to: vec![statement_id.clone()],
                status: DomainKnowledgeStatus::Active,
            },
            owner.clone(),
            None,
            Confidence::Probable,
            evidence,
        );

        let graph = builder.finish();

        for (kind, id) in [
            (NodeKind::Statement, statement_id),
            (NodeKind::Expression, expression_id.clone()),
            (NodeKind::Condition, condition_id),
            (NodeKind::Symbol, symbol_id),
            (NodeKind::Definition, definition_id),
            (NodeKind::Use, use_id),
            (NodeKind::Value, value_id),
            (NodeKind::BasicBlock, basic_block_id),
            (NodeKind::DomainKnowledge, domain_knowledge_id),
        ] {
            let node = graph
                .nodes
                .iter()
                .find(|node| node.node_id == id)
                .unwrap_or_else(|| panic!("missing {kind:?} node {id}"));
            assert_eq!(node.kind, kind);
            assert_eq!(node.owner, owner);
            assert!(!node.evidence.is_empty());
            assert!(node.evidence.iter().all(|evidence| {
                evidence.source_span == Some(span)
                    && evidence.content_hash.as_deref() == Some("sha256:test-source")
                    && evidence.syntax.is_some()
            }));
            assert!(
                graph
                    .indexes
                    .nodes_by_kind
                    .get(&kind)
                    .is_some_and(|ids| ids.contains(&id))
            );
            assert!(
                graph
                    .indexes
                    .owner_to_nodes
                    .get(&callable_id)
                    .is_some_and(|ids| ids.contains(&id))
            );
        }

        let domain_node = graph
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::DomainKnowledge)
            .expect("domain knowledge node");
        assert_eq!(domain_node.span, None);
        assert_eq!(domain_node.confidence, Confidence::Probable);

        let spanned_nodes = graph
            .nodes
            .iter()
            .filter(|node| node.kind != NodeKind::DomainKnowledge)
            .collect::<Vec<_>>();
        assert!(spanned_nodes.iter().all(|node| node.span == Some(span)));
        assert!(
            spanned_nodes
                .iter()
                .all(|node| node.confidence == Confidence::Exact)
        );

        let expression_node = graph
            .nodes
            .iter()
            .find(|node| node.node_id == expression_id)
            .expect("expression node");
        let NodeFact::Expression(expression) = &expression_node.fact else {
            panic!("expected expression fact");
        };
        assert_eq!(expression.original_text.as_deref(), Some("25"));
        assert_eq!(expression.normalized.canonical.as_deref(), Some("25"));
        assert_eq!(
            expression.normalized.literal,
            Some(ValueLiteral::Integer("25".to_string()))
        );
    }

    #[test]
    fn refresh_provenance_backfills_derived_fact_evidence() {
        let artifact_id = test_id("artifact:main");
        let span = span(30, 40);
        let mut builder = ProgramSupergraphBuilder::new("repo", "python");
        builder.add_artifact(
            Artifact {
                artifact_id: artifact_id.clone(),
                path: Sym::new("src/main.py"),
                module_path: Sym::new("main"),
                content_hash: Some(Sym::new("sha256:source-v1")),
            },
            Vec::new(),
        );

        let mut graph = builder.finish();
        let diagnostic_id = diagnostic_id("unsupported construct", Some(span));
        graph.nodes.push(GraphNode {
            node_id: diagnostic_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Diagnostic,
            owner: source_owner(Some(artifact_id.clone()), None, None),
            span: Some(span),
            confidence: Confidence::Unknown,
            uncertainty: Uncertainty::Exact,
            evidence: vec![Evidence {
                kind: EvidenceKind::Inference,
                summary: Sym::new("derived diagnostic"),
                source_id: None,
                source_span: None,
                content_hash: None,
                syntax: None,
            }],
            fact: NodeFact::Diagnostic(Diagnostic {
                diagnostic_id,
                kind: DiagnosticKind::UnsupportedSyntax,
                severity: Severity::Warning,
                message: "unsupported construct".to_string(),
                artifact_id: Some(artifact_id.clone()),
                span: Some(span),
                related: Vec::new(),
            }),
        });

        refresh_provenance(&mut graph);

        let derived = graph
            .nodes
            .iter()
            .find(|node| node.node_id != artifact_id)
            .expect("derived node");
        let evidence = derived.evidence.first().expect("derived evidence");
        assert_eq!(evidence.source_id, Some(artifact_id));
        assert_eq!(evidence.source_span, Some(span));
        assert_eq!(evidence.content_hash.as_deref(), Some("sha256:source-v1"));
    }

    #[test]
    fn stable_subject_references_survive_callable_fact_payload_changes() {
        let v1 = graph_with_callable_body(span(10, 40));
        let v2 = graph_with_callable_body(span(10, 80));

        let callable_subject = callable_id("main.calculate_risk");
        let caller_subject = callable_id("main.create_incident");
        let call_site_subject = call_site_id(
            test_id("artifact:main"),
            caller_subject,
            "calculate_risk",
            span(100, 114),
        );

        let callable_v1 = node_by_id(&v1, callable_subject);
        let callable_v2 = node_by_id(&v2, callable_subject);
        assert_eq!(callable_v1.node_id, callable_v2.node_id);
        assert_ne!(callable_v1.fact_id, callable_v2.fact_id);
        assert_ne!(callable_v1.payload_hash, callable_v2.payload_hash);

        let call_v1 = calls_edge(&v1);
        let call_v2 = calls_edge(&v2);
        assert_eq!(call_v1.edge_id, call_v2.edge_id);
        assert_eq!(call_v1.source_id, call_site_subject);
        assert_eq!(
            call_v1.target_id,
            Some(callable_subject)
        );

        let EdgeFact::Calls(calls_v2) = &call_v2.fact else {
            panic!("expected calls edge");
        };
        assert_eq!(calls_v2.caller_callable_id, caller_subject);
        assert_eq!(calls_v2.call_site_id, call_site_subject);
        assert_eq!(
            calls_v2.callee_callable_id,
            Some(callable_subject)
        );

        let trace = v2
            .edges
            .iter()
            .find_map(|edge| match &edge.fact {
                EdgeFact::TracesTo(trace) => Some(trace),
                _ => None,
            })
            .expect("requirement trace");
        assert_eq!(trace.code_fact_id, callable_subject);
        assert!(
            v2.indexes
                .calls_by_concrete_target
                .get(&callable_subject)
                .is_some_and(|edge_ids| edge_ids.contains(&call_v2.edge_id))
        );
        assert!(
            v2.indexes
                .calls_by_caller
                .get(&caller_subject)
                .is_some_and(|edge_ids| edge_ids.contains(&call_v2.edge_id))
        );
    }

    #[test]
    fn serialized_supergraph_preserves_source_span_index_keys_and_defaulted_fact_identity() {
        let mut graph = graph_with_callable_body(span(10, 40));
        let callable_subject = callable_id("main.calculate_risk");
        let callable_node = graph
            .nodes
            .iter_mut()
            .find(|node| node.node_id == callable_subject)
            .expect("callable node");
        callable_node.fact_id = None;
        callable_node.payload_hash = None;

        let json = serde_json::to_string(&graph).expect("serialize graph");
        assert!(!json.contains("\"indexes\""), "indexes are rebuilt on load, not serialized");
        let key = serde_json::to_string(&SourceSpanIndexKey {
            artifact_id: Some(test_id("artifact:main")),
            span: span(10, 25),
        })
        .expect("serialize span key");
        assert!(
            key.contains("\\u001f"),
            "source span index keys should use the documented unit separator encoding"
        );

        let graph: ProgramSupergraph =
            serde_json::from_str(&json).expect("deserialize serialized graph");
        assert!(
            graph
                .indexes
                .source_span_to_nodes
                .contains_key(&SourceSpanIndexKey {
                    artifact_id: Some(test_id("artifact:main")),
                    span: span(10, 25),
                }),
            "source span index keys should roundtrip through JSON map keys"
        );

        let mut value = serde_json::to_value(&graph).expect("graph to JSON value");
        let node = value["nodes"]
            .as_array_mut()
            .expect("nodes array")
            .iter_mut()
            .find(|node| node["node_id"] == callable_subject.to_string())
            .expect("callable node JSON");
        node.as_object_mut().expect("node object").remove("fact_id");
        node.as_object_mut()
            .expect("node object")
            .remove("payload_hash");

        let graph: ProgramSupergraph =
            serde_json::from_value(value).expect("deserialize graph without fact identity fields");
        let migrated = node_by_id(&graph, callable_subject);
        assert_eq!(migrated.fact_id, None);
        assert_eq!(migrated.payload_hash, None);
    }

    #[test]
    fn uncertainty_classifications_cover_sg013_acceptance_examples() {
        let artifact_id = test_id("artifact:main");
        let scope_id = test_id("scope:module");
        let caller_id = test_id("callable:main.run");
        let call_site_id = test_id("call-site:dynamic");
        let span = span(200, 220);
        let owner = source_owner(
            Some(artifact_id.clone()),
            Some(scope_id.clone()),
            Some(caller_id.clone()),
        );
        let mut builder = ProgramSupergraphBuilder::new("repo", "python");

        builder.add_artifact(
            Artifact {
                artifact_id: artifact_id.clone(),
                path: Sym::new("main.py"),
                module_path: Sym::new("main"),
                content_hash: Some(Sym::new("sha256:sg013")),
            },
            Vec::new(),
        );
        builder.add_statement(
            Statement {
                statement_id: test_id("statement:probable"),
                callable_id: caller_id.clone(),
                parent_statement_id: None,
                kind: StatementKind::Expression,
                ordinal: 0,
                child_statement_ids: Vec::new(),
                expression_ids: Vec::new(),
                control_effects: Vec::new(),
            },
            owner.clone(),
            Some(span),
            Confidence::Probable,
            test_evidence("probable normalized statement"),
        );
        builder.add_symbol(
            Symbol {
                symbol_id: test_id("symbol:ambiguous"),
                scope_id: scope_id.clone(),
                name: Sym::new("handler"),
                kind: SymbolKind::Local,
                binding_id: None,
                resolution: Resolution::Ambiguous,
            },
            owner.clone(),
            Some(span),
            Confidence::Unknown,
            test_evidence("ambiguous symbol resolution"),
        );
        builder.add_external_target(
            ExternalTarget {
                external_target_id: test_id("external:service"),
                ecosystem: Sym::new("python"),
                package_name: Some(Sym::new("service")),
                package_version: None,
                module_path: Some(Sym::new("service")),
                qualified_name: Sym::new("service.call"),
                member_path: None,
                target_kind: ExternalTargetKind::Function,
                source: Sym::new("imported package"),
            },
            Confidence::Exact,
            test_evidence("external target"),
        );
        builder.add_calls(
            Calls {
                caller_callable_id: caller_id.clone(),
                callee_callable_id: None,
                external_target_id: None,
                unresolved_target: Some(Sym::new("unknown_target")),
                call_site_id: call_site_id.clone(),
                kind: CallEdgeKind::PossibleDynamic,
                resolution: Resolution::Unresolved,
            },
            owner.clone(),
            Some(span),
            Confidence::Unknown,
            test_evidence("unresolved call retained"),
        );
        builder.add_calls(
            Calls {
                caller_callable_id: caller_id.clone(),
                callee_callable_id: Some(test_id("callable:maybe")),
                external_target_id: None,
                unresolved_target: None,
                call_site_id: call_site_id.clone(),
                kind: CallEdgeKind::PossibleDynamic,
                resolution: Resolution::Possible,
            },
            owner.clone(),
            Some(span),
            Confidence::Probable,
            test_evidence("dynamic dispatch possible target"),
        );
        let alias_uncertainty_edge_id = builder.add_parameter_out(
            test_id("value:receiver"),
            call_site_id.clone(),
            ParameterOut {
                call_site_id: call_site_id.clone(),
                caller_callable_id: caller_id.clone(),
                callee_callable_id: test_id("callable:maybe"),
                parameter_name: Sym::new("receiver"),
                ordinal: 0,
                precision: Sym::new("sg083-parameter-out-possible-alias-mutation-summary"),
            },
            owner.clone(),
            Some(span),
            Confidence::Unknown,
            test_evidence("possible alias mutation summary retained"),
        );
        for (id, kind, message) in [
            (
                "diagnostic:ambiguous",
                DiagnosticKind::AmbiguousSymbol,
                "symbol could resolve to multiple bindings",
            ),
            (
                "diagnostic:dynamic",
                DiagnosticKind::DynamicDispatch,
                "dynamic dispatch target is possible",
            ),
            (
                "diagnostic:unsupported",
                DiagnosticKind::UnsupportedSyntax,
                "unsupported syntax retained as diagnostic",
            ),
            (
                "diagnostic:alias",
                DiagnosticKind::AliasUncertainty,
                "alias-sensitive mutation is uncertain",
            ),
        ] {
            builder.add_diagnostic(
                Diagnostic {
                    diagnostic_id: test_id(id),
                    kind,
                    severity: Severity::Warning,
                    message: message.to_string(),
                    artifact_id: Some(artifact_id.clone()),
                    span: Some(span),
                    related: vec![call_site_id.clone()],
                },
                Confidence::Unknown,
                test_evidence(message),
            );
        }
        builder.add_domain_knowledge(
            DomainKnowledge {
                domain_knowledge_id: test_id("domain:stale"),
                scope: DomainKnowledgeScope::Repository,
                summary: "legacy API contract".to_string(),
                source: DomainKnowledgeSource::Documentation,
                applies_to: vec![caller_id.clone()],
                status: DomainKnowledgeStatus::Stale,
            },
            owner,
            None,
            Confidence::Probable,
            test_evidence("stale domain knowledge"),
        );

        let graph = builder.finish();

        assert_eq!(
            node_uncertainty_by_id(&graph, artifact_id),
            Some(Uncertainty::Exact)
        );
        assert_eq!(
            node_uncertainty_by_id(&graph, test_id("statement:probable")),
            Some(Uncertainty::Probable)
        );
        assert_eq!(
            node_uncertainty_by_id(&graph, test_id("external:service")),
            Some(Uncertainty::External)
        );
        assert_eq!(
            node_uncertainty_by_id(&graph, test_id("symbol:ambiguous")),
            Some(Uncertainty::Ambiguous)
        );
        assert_eq!(
            node_uncertainty_by_id(&graph, test_id("diagnostic:unsupported")),
            Some(Uncertainty::Unsupported)
        );
        assert_eq!(
            node_uncertainty_by_id(&graph, test_id("domain:stale")),
            Some(Uncertainty::Stale)
        );
        assert!(graph.edges.iter().any(|edge| {
            matches!(&edge.fact, EdgeFact::Calls(calls) if calls.unresolved_target.is_some())
                && edge.uncertainty == Uncertainty::Unresolved
        }));
        assert!(graph.edges.iter().any(|edge| {
            matches!(&edge.fact, EdgeFact::Calls(calls) if calls.kind == CallEdgeKind::PossibleDynamic)
                && edge.uncertainty == Uncertainty::Possible
        }));
        assert_eq!(
            edge_uncertainty_by_id(&graph, alias_uncertainty_edge_id),
            Some(Uncertainty::Possible)
        );
    }


    #[test]
    fn sg120_schema_completeness_fixture_covers_all_families_and_indexes() {
        let (graph, ids, span) = sg120_schema_fixture();
        let roundtrip: ProgramSupergraph =
            serde_json::from_str(&serde_json::to_string(&graph).expect("serialize sg120 fixture"))
                .expect("deserialize sg120 fixture");

        let expected_nodes = [
            (NodeKind::Artifact, ids.artifact),
            (NodeKind::Scope, ids.scope),
            (NodeKind::Binding, ids.binding),
            (NodeKind::Callable, ids.callable),
            (NodeKind::CallSite, ids.call_site),
            (NodeKind::ExternalTarget, ids.external),
            (NodeKind::Statement, ids.statement),
            (NodeKind::Expression, ids.expression),
            (NodeKind::Condition, ids.condition),
            (NodeKind::Symbol, ids.symbol),
            (NodeKind::Definition, ids.definition),
            (NodeKind::Use, ids.use_node),
            (NodeKind::Value, ids.value_source),
            (NodeKind::BasicBlock, ids.basic_block),
            (NodeKind::DomainKnowledge, ids.domain),
            (NodeKind::ControlFlow, ids.cfg_entry),
            (NodeKind::DataFlow, ids.data_flow_node),
            (NodeKind::Requirement, ids.requirement),
            (NodeKind::Diagnostic, ids.diagnostic),
        ];
        assert_eq!(
            expected_nodes.len(),
            19,
            "all NodeKind families must be listed"
        );

        for (kind, id) in expected_nodes {
            let node = node_by_id(&roundtrip, id);
            assert_eq!(node.kind, kind);
            assert_eq!(node.kind, node_fact_kind(&node.fact));
            assert_eq!(node.node_id, id);
            assert!(!node.fact_id.is_none(), "{kind:?} should have a fact id");
            assert!(
                !node.payload_hash.is_none(),
                "{kind:?} should have a payload hash"
            );
            assert!(!node.evidence.is_empty(), "{kind:?} should carry evidence");
            assert!(
                node.evidence.iter().all(|evidence| {
                    evidence.source_id == Some(ids.artifact)
                        && evidence.content_hash.as_deref() == Some("sha256:sg120")
                }),
                "{kind:?} evidence should retain provenance"
            );
            assert!(
                roundtrip
                    .indexes
                    .nodes_by_kind
                    .get(&kind)
                    .is_some_and(|node_ids| node_ids.iter().any(|node_id| node_id == &id)),
                "{kind:?} should be indexed by family"
            );
            assert!(
                roundtrip.indexes.node_position_by_id.contains_key(&id),
                "{kind:?} should be indexed by stable subject id"
            );
        }

        let expected_edges = [
            (EdgeKind::Contains, ids.contains),
            (EdgeKind::Binds, ids.binds),
            (EdgeKind::ResolvesTo, ids.resolves_to),
            (EdgeKind::Calls, ids.calls),
            (EdgeKind::ControlFlow, ids.control_flow),
            (EdgeKind::Controls, ids.controls),
            (EdgeKind::Defines, ids.defines),
            (EdgeKind::Uses, ids.uses),
            (EdgeKind::DataFlow, ids.data_flow),
            (EdgeKind::ParameterIn, ids.parameter_in),
            (EdgeKind::ReturnsTo, ids.returns_to),
            (EdgeKind::ParameterOut, ids.parameter_out),
            (EdgeKind::ThrowsTo, ids.throws_to),
            (EdgeKind::DecomposesTo, ids.decomposes_to),
            (EdgeKind::Conditions, ids.conditions),
            (EdgeKind::Orders, ids.orders),
            (EdgeKind::TracesTo, ids.traces_to),
            (
                EdgeKind::DependsOnDomainKnowledge,
                ids.depends_on_domain,
            ),
        ];
        assert_eq!(
            expected_edges.len(),
            18,
            "all EdgeKind families must be listed"
        );

        for (kind, id) in expected_edges {
            let edge = edge_by_id(&roundtrip, id);
            assert_eq!(edge.kind, kind);
            assert_eq!(edge.kind, edge_fact_kind(&edge.fact));
            assert_eq!(edge.edge_id, id);
            assert!(edge.source_id.hash.get() != 0, "{kind:?} should have a source");
            assert!(!edge.fact_id.is_none(), "{kind:?} should have a fact id");
            assert!(
                !edge.payload_hash.is_none(),
                "{kind:?} should have a payload hash"
            );
            assert!(!edge.evidence.is_empty(), "{kind:?} should carry evidence");
            assert!(
                roundtrip
                    .indexes
                    .edges_by_kind
                    .get(&kind)
                    .is_some_and(|edge_ids| edge_ids.iter().any(|edge_id| edge_id == &id)),
                "{kind:?} should be indexed by family"
            );
            assert!(
                roundtrip.indexes.edge_position_by_id.contains_key(&id),
                "{kind:?} should be indexed by stable subject id"
            );
            assert!(
                crate::supergraph::views::indexed_outgoing_edges_by_kind(&roundtrip, edge.source_id, kind)
                    .iter()
                    .any(|candidate| candidate.edge_id == id),
                "{kind:?} should be indexed by source and family"
            );
            if let Some(target_id) = edge.target_id {
                assert!(
                    crate::supergraph::views::indexed_incoming_edges_by_kind(&roundtrip, target_id, kind)
                        .iter()
                        .any(|candidate| candidate.edge_id == id),
                    "{kind:?} should be indexed by target and family"
                );
            }
        }

        assert!(
            roundtrip
                .indexes
                .source_span_to_nodes
                .get(&SourceSpanIndexKey {
                    artifact_id: Some(ids.artifact.clone()),
                    span,
                })
                .is_some_and(|node_ids| node_ids.contains(&ids.statement)
                    && node_ids.contains(&ids.expression)
                    && node_ids.contains(&ids.value_source)),
            "source-span index should cover spanned schema facts"
        );
        assert!(
            roundtrip
                .indexes
                .artifact_to_nodes
                .get(&ids.artifact)
                .is_some_and(|node_ids| node_ids.contains(&ids.callable)
                    && node_ids.contains(&ids.requirement)),
            "artifact ownership index should cover owned facts"
        );
        assert!(
            roundtrip
                .indexes
                .callable_to_nodes
                .get(&ids.callable)
                .is_some_and(|node_ids| node_ids.contains(&ids.statement)
                    && node_ids.contains(&ids.value_source)),
            "callable ownership index should cover callable-owned facts"
        );
        assert!(
            roundtrip
                .indexes
                .owner_to_edges
                .get(&ids.callable)
                .is_some_and(|edge_ids| edge_ids.contains(&ids.data_flow)
                    && edge_ids.contains(&ids.control_flow)),
            "edge ownership index should cover callable-owned analysis edges"
        );
        assert_eq!(
            roundtrip.indexes.symbol_to_definitions.get(&ids.symbol),
            Some(&[ids.definition.clone()][..])
        );
        assert_eq!(
            roundtrip.indexes.symbol_to_uses.get(&ids.symbol),
            Some(&[ids.use_node.clone()][..])
        );
        assert_eq!(
            roundtrip.indexes.calls_by_caller.get(&ids.callable),
            Some(&[ids.calls.clone()][..])
        );
        assert_eq!(
            roundtrip
                .indexes
                .calls_by_concrete_target
                .get(&ids.external),
            Some(&[ids.calls.clone()][..])
        );
        assert_eq!(
            roundtrip.indexes.call_site_to_calls.get(&ids.call_site),
            Some(&[ids.calls.clone()][..])
        );
        assert_eq!(
            roundtrip
                .indexes
                .caller_to_concrete_target_calls
                .get(&ids.callable)
                .and_then(|targets| targets.get(&ids.external)),
            Some(&vec![ids.calls.clone()])
        );
        assert_eq!(
            roundtrip.indexes.requirement_to_code.get(&ids.requirement),
            Some(&[ids.statement.clone()][..])
        );
        assert_eq!(
            roundtrip.indexes.code_to_requirements.get(&ids.statement),
            Some(&[ids.requirement.clone()][..])
        );
        assert_eq!(
            roundtrip
                .indexes
                .requirement_to_domain_knowledge
                .get(&ids.requirement),
            Some(&[ids.domain.clone()][..])
        );
        assert_eq!(
            roundtrip
                .indexes
                .domain_knowledge_to_requirements
                .get(&ids.domain),
            Some(&[ids.requirement.clone()][..])
        );
        assert!(
            !roundtrip
                .indexes
                .requirement_to_code
                .get(&ids.requirement)
                .is_some_and(|code_ids| code_ids.contains(&ids.domain)),
            "domain dependencies must not roundtrip as trace evidence"
        );
        assert!(
            roundtrip
                .indexes
                .nodes_by_uncertainty
                .get(&Uncertainty::Stale)
                .is_some_and(|node_ids| node_ids.contains(&ids.domain)),
            "uncertainty buckets should include stale domain facts"
        );
        assert!(
            roundtrip
                .indexes
                .edges_by_uncertainty
                .get(&Uncertainty::Possible)
                .is_some_and(|edge_ids| edge_ids.contains(&ids.parameter_out)),
            "uncertainty buckets should include possible edge facts"
        );
    }

    #[test]
    fn sg120_defaulted_serialization_fields_remain_compatible() {
        let (graph, ids, _) = sg120_schema_fixture();
        let mut value = serde_json::to_value(&graph).expect("graph to value");

        let node = value["nodes"]
            .as_array_mut()
            .expect("nodes array")
            .iter_mut()
            .find(|node| node["node_id"] == ids.requirement.to_string())
            .expect("requirement node JSON");
        let node_object = node.as_object_mut().expect("node object");
        node_object.remove("fact_id");
        node_object.remove("payload_hash");
        node_object.remove("uncertainty");

        let edge = value["edges"]
            .as_array_mut()
            .expect("edges array")
            .iter_mut()
            .find(|edge| edge["edge_id"] == ids.traces_to.to_string())
            .expect("trace edge JSON");
        let edge_object = edge.as_object_mut().expect("edge object");
        edge_object.remove("fact_id");
        edge_object.remove("payload_hash");
        edge_object.remove("uncertainty");

        let migrated: ProgramSupergraph =
            serde_json::from_value(value).expect("deserialize graph with omitted defaults");
        let requirement = node_by_id(&migrated, ids.requirement);
        assert_eq!(requirement.fact_id, None);
        assert_eq!(requirement.payload_hash, None);
        assert_eq!(requirement.uncertainty, Uncertainty::Exact);

        let trace = edge_by_id(&migrated, ids.traces_to);
        assert_eq!(trace.fact_id, None);
        assert_eq!(trace.payload_hash, None);
        assert_eq!(trace.uncertainty, Uncertainty::Exact);
    }

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start as u32,
            end_byte: end as u32,
            start_row: 1,
            start_column: start as u32,
            end_row: 1,
            end_column: end as u32,
        }
    }

    #[allow(dead_code)]
    #[derive(Debug)]
    struct Sg120FixtureIds {
        artifact: NodeId,
        scope: NodeId,
        binding: NodeId,
        callable: NodeId,
        call_site: NodeId,
        external: NodeId,
        statement: NodeId,
        expression: NodeId,
        condition: NodeId,
        symbol: NodeId,
        definition: NodeId,
        use_node: NodeId,
        value_source: NodeId,
        value_formal: NodeId,
        value_argument: NodeId,
        value_return: NodeId,
        value_call_result: NodeId,
        value_mutable_state: NodeId,
        value_exception: NodeId,
        basic_block: NodeId,
        domain: NodeId,
        cfg_entry: NodeId,
        cfg_exit: NodeId,
        data_flow_node: NodeId,
        requirement: NodeId,
        child_requirement: NodeId,
        condition_requirement: NodeId,
        diagnostic: NodeId,
        contains: EdgeId,
        binds: EdgeId,
        resolves_to: EdgeId,
        calls: EdgeId,
        control_flow: EdgeId,
        controls: EdgeId,
        defines: EdgeId,
        uses: EdgeId,
        data_flow: EdgeId,
        parameter_in: EdgeId,
        returns_to: EdgeId,
        parameter_out: EdgeId,
        throws_to: EdgeId,
        decomposes_to: EdgeId,
        conditions: EdgeId,
        orders: EdgeId,
        traces_to: EdgeId,
        depends_on_domain: EdgeId,
    }

    fn sg120_schema_fixture() -> (ProgramSupergraph, Sg120FixtureIds, SourceSpan) {
        let span = span(10, 20);
        let artifact = test_id("artifact:sg120");
        let scope = test_id("scope:sg120.module");
        let binding = test_id("binding:sg120.handler");
        let callable = test_id("callable:sg120.handler");
        let call_site = test_id("call-site:sg120.emit");
        let external = test_id("external:sg120.service.emit");
        let statement = test_id("statement:sg120.assign");
        let expression = test_id("expression:sg120.literal");
        let condition = test_id("condition:sg120.guard");
        let symbol = test_id("symbol:sg120.batch_size");
        let definition = test_id("definition:sg120.batch_size");
        let use_node = test_id("use:sg120.batch_size");
        let value_source = test_id("value:sg120.literal");
        let value_formal = test_id("value:sg120.formal");
        let value_argument = test_id("value:sg120.argument");
        let value_return = test_id("value:sg120.return");
        let value_call_result = test_id("value:sg120.call-result");
        let value_mutable_state = test_id("value:sg120.mutable-state");
        let value_exception = test_id("value:sg120.exception");
        let basic_block = test_id("basic-block:sg120.0");
        let domain = test_id("domain-knowledge:sg120");
        let cfg_entry = test_id("cfg:sg120.entry");
        let cfg_exit = test_id("cfg:sg120.exit");
        let data_flow_node = test_id("dfg:sg120.use");
        let requirement = test_id("requirement:sg120.parent");
        let child_requirement = test_id("requirement:sg120.child");
        let condition_requirement = test_id("requirement:sg120.condition");
        let diagnostic = test_id("diagnostic:sg120.unsupported");
        let owner = source_owner(
            Some(artifact.clone()),
            Some(scope.clone()),
            Some(callable.clone()),
        );
        let artifact_owner = source_owner(Some(artifact.clone()), None, None);
        let scope_owner = source_owner(Some(artifact.clone()), Some(scope.clone()), None);

        let mut builder = ProgramSupergraphBuilder::new("repo", "python");
        builder.insert_node(schema_node(
            artifact.clone(),
            NodeKind::Artifact,
            artifact_owner.clone(),
            None,
            Confidence::Exact,
            schema_evidence(artifact, "artifact source", None),
            NodeFact::Artifact(Artifact {
                artifact_id: artifact.clone(),
                path: Sym::new("sg120.py"),
                module_path: Sym::new("sg120"),
                content_hash: Some(Sym::new("sha256:sg120")),
            }),
        ));
        builder.insert_node(schema_node(
            scope.clone(),
            NodeKind::Scope,
            artifact_owner.clone(),
            None,
            Confidence::Exact,
            schema_evidence(artifact, "module scope", None),
            NodeFact::Scope(Box::new(Scope {
                scope_id: scope.clone(),
                parent_scope_id: None,
                artifact_id: artifact.clone(),
                kind: ScopeKind::Module,
                variant: ScopeVariant::FileModule,
                language_variant: Some(Sym::new("python:module")),
                binding_behavior: ScopeBindingBehavior::Boundary,
                owner_callable_id: None,
                span: None,
            })),
        ));
        builder.insert_node(schema_node(
            binding.clone(),
            NodeKind::Binding,
            scope_owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "binding", Some(span)),
            NodeFact::Binding(Binding {
                binding_id: binding.clone(),
                scope_id: scope.clone(),
                name: Sym::new("handler"),
                kind: BindingKind::Function,
                target: BindingTarget::Callable(callable.clone()),
                span,
            }),
        ));
        builder.insert_node(schema_node(
            callable.clone(),
            NodeKind::Callable,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "callable", Some(span)),
            NodeFact::Callable(Box::new(Callable {
                callable_id: callable.clone(),
                kind: CallableKind::Function,
                name: Some(Sym::new("handler")),
                qualified_name: Sym::new("sg120.handler"),
                artifact_id: artifact.clone(),
                declaration_span: span,
                body_span: Some(span),
                signature: Signature {
                    parameters: vec![Sym::new("items")],
                    return_annotation: Some(Sym::new("int")),
                },
                scope_id: scope.clone(),
                attributes: vec![Sym::new("entrypoint")],
                incoming_local_call_count: 0,
                external_invocation_metadata: vec![Sym::new("route")],
            })),
        ));
        builder.insert_node(schema_node(
            call_site.clone(),
            NodeKind::CallSite,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "call site", Some(span)),
            NodeFact::CallSite(Box::new(CallSite {
                call_site_id: call_site.clone(),
                artifact_id: artifact.clone(),
                enclosing_callable_id: callable.clone(),
                span,
                callee_expression: Sym::new("emit"),
                argument_shape: ArgumentShape {
                    positional_count: 1,
                    named_arguments: Vec::new(),
                },
                dispatch_kind: DispatchKind::Direct,
                context: crate::ast::CallContext::Body,
            })),
        ));
        builder.insert_node(schema_node(
            external.clone(),
            NodeKind::ExternalTarget,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "external target", Some(span)),
            NodeFact::ExternalTarget(ExternalTarget {
                external_target_id: external.clone(),
                ecosystem: Sym::new("python"),
                package_name: Some(Sym::new("service")),
                package_version: None,
                module_path: Some(Sym::new("service")),
                qualified_name: Sym::new("service.emit"),
                member_path: None,
                target_kind: ExternalTargetKind::Function,
                source: Sym::new("import"),
            }),
        ));
        builder.insert_node(schema_node(
            statement.clone(),
            NodeKind::Statement,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "statement", Some(span)),
            NodeFact::Statement(Box::new(Statement {
                statement_id: statement.clone(),
                callable_id: callable.clone(),
                parent_statement_id: None,
                kind: StatementKind::Assignment,
                ordinal: 0,
                child_statement_ids: Vec::new(),
                expression_ids: vec![expression.clone()],
                control_effects: Vec::new(),
            })),
        ));
        builder.insert_node(schema_node(
            expression.clone(),
            NodeKind::Expression,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "expression", Some(span)),
            NodeFact::Expression(Box::new(Expression {
                expression_id: expression.clone(),
                callable_id: callable.clone(),
                statement_id: Some(statement.clone()),
                parent_expression_id: None,
                kind: ExpressionKind::Literal,
                ordinal: 0,
                child_expression_ids: Vec::new(),
                symbol_id: Some(symbol.clone()),
                value_id: Some(value_source.clone()),
                original_text: Some(Sym::new("25")),
                normalized: NormalizedExpression {
                    canonical: Some(Sym::new("25")),
                    literal: Some(ValueLiteral::Integer("25".to_string())),
                    ..Default::default()
                },
            })),
        ));
        builder.insert_node(schema_node(
            condition.clone(),
            NodeKind::Condition,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "condition", Some(span)),
            NodeFact::Condition(Box::new(Condition {
                condition_id: condition.clone(),
                callable_id: callable.clone(),
                statement_id: Some(statement.clone()),
                expression_id: Some(expression.clone()),
                kind: ConditionKind::Guard,
                controlled_statement_ids: vec![statement.clone()],
                outcome_labels: vec![Sym::new("true")],
                regions: Vec::new(),
                continuation: None,
                fallthrough: FallthroughBehavior::Conditional,
            })),
        ));
        builder.insert_node(schema_node(
            symbol.clone(),
            NodeKind::Symbol,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "symbol", Some(span)),
            NodeFact::Symbol(Symbol {
                symbol_id: symbol.clone(),
                scope_id: scope.clone(),
                name: Sym::new("batch_size"),
                kind: SymbolKind::Local,
                binding_id: Some(binding.clone()),
                resolution: Resolution::Exact,
            }),
        ));
        builder.insert_node(schema_node(
            definition.clone(),
            NodeKind::Definition,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "definition", Some(span)),
            NodeFact::Definition(Definition {
                definition_id: definition.clone(),
                callable_id: callable.clone(),
                symbol_id: Some(symbol.clone()),
                value_id: Some(value_source.clone()),
                kind: DefinitionKind::Assignment,
                name: Some(Sym::new("batch_size")),
            }),
        ));
        builder.insert_node(schema_node(
            use_node.clone(),
            NodeKind::Use,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "use", Some(span)),
            NodeFact::Use(Use {
                use_id: use_node.clone(),
                callable_id: callable.clone(),
                symbol_id: Some(symbol.clone()),
                value_id: Some(value_source.clone()),
                kind: UseKind::Read,
                name: Some(Sym::new("batch_size")),
            }),
        ));
        for (id, kind, role, symbol_id, expression_id, call_site_id, name, ordinal, state_of) in [
            (
                value_source.clone(),
                ValueKind::Literal,
                ValueRole::Unknown,
                Some(symbol.clone()),
                Some(expression.clone()),
                None,
                Some("batch_size".to_string()),
                None,
                None,
            ),
            (
                value_formal.clone(),
                ValueKind::Parameter,
                ValueRole::FormalParameter,
                Some(symbol.clone()),
                None,
                None,
                Some("items".to_string()),
                Some(0),
                None,
            ),
            (
                value_argument.clone(),
                ValueKind::Argument,
                ValueRole::Argument,
                None,
                Some(expression.clone()),
                Some(call_site.clone()),
                Some("items".to_string()),
                Some(0),
                None,
            ),
            (
                value_return.clone(),
                ValueKind::Return,
                ValueRole::ReturnValue,
                None,
                Some(expression.clone()),
                None,
                None,
                None,
                None,
            ),
            (
                value_call_result.clone(),
                ValueKind::CallResult,
                ValueRole::CallResult,
                None,
                Some(expression.clone()),
                Some(call_site.clone()),
                None,
                None,
                None,
            ),
            (
                value_mutable_state.clone(),
                ValueKind::Argument,
                ValueRole::MutableArgumentState,
                None,
                None,
                Some(call_site.clone()),
                Some("items".to_string()),
                Some(0),
                Some(value_argument.clone()),
            ),
            (
                value_exception.clone(),
                ValueKind::Exception,
                ValueRole::ExceptionalValue,
                None,
                None,
                Some(call_site.clone()),
                None,
                None,
                None,
            ),
        ] {
            builder.insert_node(schema_node(
                id.clone(),
                NodeKind::Value,
                owner.clone(),
                Some(span),
                Confidence::Exact,
                schema_evidence(artifact, "value", Some(span)),
                NodeFact::Value(Value {
                    value_id: id,
                    callable_id: Some(callable.clone()),
                    kind,
                    role,
                    symbol_id,
                    expression_id,
                    call_site_id,
                    name: name.map(Sym::from),
                    ordinal,
                    state_of_value_id: state_of,
                    type_hint: Some(Sym::new("int")),
                    literal: None,
                }),
            ));
        }
        builder.insert_node(schema_node(
            basic_block.clone(),
            NodeKind::BasicBlock,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "basic block", Some(span)),
            NodeFact::BasicBlock(Box::new(BasicBlock {
                basic_block_id: basic_block.clone(),
                callable_id: callable.clone(),
                kind: BasicBlockKind::StraightLine,
                ordinal: 0,
                statement_ids: vec![statement.clone()],
                entry_node_id: Some(cfg_entry.clone()),
                exit_node_id: Some(cfg_exit.clone()),
            })),
        ));
        builder.insert_node(schema_node(
            domain.clone(),
            NodeKind::DomainKnowledge,
            artifact_owner.clone(),
            None,
            Confidence::Probable,
            schema_evidence(artifact, "domain knowledge", None),
            NodeFact::DomainKnowledge(Box::new(DomainKnowledge {
                domain_knowledge_id: domain.clone(),
                scope: DomainKnowledgeScope::Repository,
                summary: "batch sizes are externally visible".to_string(),
                source: DomainKnowledgeSource::Documentation,
                applies_to: vec![requirement.clone()],
                status: DomainKnowledgeStatus::Stale,
            })),
        ));
        for (id, role, label) in [
            (cfg_entry.clone(), ControlFlowNodeRole::Entry, "entry"),
            (cfg_exit.clone(), ControlFlowNodeRole::Exit, "exit"),
        ] {
            builder.insert_node(schema_node(
                id.clone(),
                NodeKind::ControlFlow,
                owner.clone(),
                Some(span),
                Confidence::Exact,
                schema_evidence(artifact, "cfg node", Some(span)),
                NodeFact::ControlFlow(ControlFlowNode {
                    cfg_node_id: id,
                    callable_id: callable.clone(),
                    role,
                    label: Sym::from(label.to_string()),
                    semantic_kind: Some(Sym::new("sg120")),
                }),
            ));
        }
        builder.insert_node(schema_node(
            data_flow_node.clone(),
            NodeKind::DataFlow,
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "data-flow node", Some(span)),
            NodeFact::DataFlow(DataFlowNode {
                data_flow_node_id: data_flow_node.clone(),
                callable_id: callable.clone(),
                role: DataFlowNodeRole::Use,
                name: Some(Sym::new("batch_size")),
                text: Sym::new("batch_size"),
                semantic_kind: Some(Sym::new("legacy-use")),
            }),
        ));
        for (id, kind, title) in [
            (
                requirement.clone(),
                RequirementKind::Boundary,
                "Boundary requirement",
            ),
            (
                child_requirement.clone(),
                RequirementKind::Definition,
                "Child requirement",
            ),
            (
                condition_requirement.clone(),
                RequirementKind::Condition,
                "Condition requirement",
            ),
        ] {
            builder.insert_node(schema_node(
                id.clone(),
                NodeKind::Requirement,
                artifact_owner.clone(),
                None,
                Confidence::Exact,
                schema_evidence(artifact, "requirement", None),
                NodeFact::Requirement(Requirement {
                    requirement_id: id,
                    kind,
                    title: Sym::from(title.to_string()),
                    summary: Sym::new("Schema completeness fixture requirement."),
                    source_rule: Sym::new("sg120-schema-completeness"),
                    path_conditions: Vec::new(),
                }),
            ));
        }
        builder.insert_node(schema_node(
            diagnostic.clone(),
            NodeKind::Diagnostic,
            artifact_owner.clone(),
            Some(span),
            Confidence::Unknown,
            schema_evidence(artifact, "diagnostic", Some(span)),
            NodeFact::Diagnostic(Diagnostic {
                diagnostic_id: diagnostic.clone(),
                kind: DiagnosticKind::UnsupportedSyntax,
                severity: Severity::Warning,
                message: "unsupported fixture construct".to_string(),
                artifact_id: Some(artifact.clone()),
                span: Some(span),
                related: vec![statement.clone()],
            }),
        ));

        let contains = stable_edge_id(id_parts!["contains", artifact, scope]);
        let binds = stable_edge_id(id_parts!["binds", scope, binding]);
        let resolves_to = stable_edge_id(id_parts!["resolves-to", binding, callable]);
        let calls = stable_edge_id(id_parts!["calls", callable, call_site, external]);
        let control_flow = stable_edge_id(id_parts!["control-flow", cfg_entry, cfg_exit]);
        let controls = stable_edge_id(id_parts!["controls", condition, statement]);
        let defines = stable_edge_id(id_parts!["defines", definition, symbol]);
        let uses = stable_edge_id(id_parts!["uses", use_node, symbol]);
        let data_flow = stable_edge_id(id_parts!["data-flow", value_source, value_call_result]);
        let parameter_in = stable_edge_id(id_parts!["parameter-in", value_argument, value_formal]);
        let returns_to = stable_edge_id(id_parts!["returns-to", value_return, value_call_result]);
        let parameter_out = stable_edge_id(id_parts!["parameter-out", value_formal, value_mutable_state]);
        let throws_to = stable_edge_id(id_parts!["throws-to", value_exception, cfg_exit]);
        let decomposes_to = stable_edge_id(id_parts!["decomposes-to", requirement, child_requirement]);
        let conditions = stable_edge_id(id_parts!["conditions", condition_requirement, child_requirement]);
        let orders = stable_edge_id(id_parts!["orders", requirement, child_requirement]);
        let traces_to = stable_edge_id(id_parts!["traces-to", requirement, statement]);
        let depends_on_domain = stable_edge_id(id_parts!["depends-on-domain-knowledge", requirement, domain]);

        builder.add_contains(
            artifact.clone(),
            scope.clone(),
            artifact_owner.clone(),
            schema_evidence(artifact, "contains", None),
        );
        builder.add_binds(
            scope.clone(),
            binding.clone(),
            scope_owner.clone(),
            schema_evidence(artifact, "binds", Some(span)),
        );
        builder.add_resolves_to_with_resolution(
            binding.clone(),
            callable.clone(),
            Resolution::Exact,
            scope_owner.clone(),
            Confidence::Exact,
            schema_evidence(artifact, "resolves", Some(span)),
        );
        builder.add_calls(
            Calls {
                caller_callable_id: callable.clone(),
                callee_callable_id: None,
                external_target_id: Some(external.clone()),
                unresolved_target: None,
                call_site_id: call_site.clone(),
                kind: CallEdgeKind::External,
                resolution: Resolution::External,
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "calls", Some(span)),
        );
        builder.add_control_flow(
            cfg_entry.clone(),
            cfg_exit.clone(),
            ControlFlow {
                callable_id: callable.clone(),
                flow_kind: ControlFlowKind::Exit,
                outcome: super::super::schema::ControlFlowOutcome::Exit,
                branch_arm: None,
                precision: Sym::new("sg120-exact-cfg"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "control flow", Some(span)),
        );
        builder.add_controls(
            Controls {
                callable_id: callable.clone(),
                condition_id: condition.clone(),
                controlled_id: statement.clone(),
                precision: Sym::new("sg120-exact-control"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "controls", Some(span)),
        );
        builder.add_defines(
            definition.clone(),
            symbol.clone(),
            Defines {
                callable_id: callable.clone(),
                definition_id: definition.clone(),
                name: Sym::new("batch_size"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "defines", Some(span)),
        );
        builder.add_uses(
            use_node.clone(),
            symbol.clone(),
            Uses {
                callable_id: callable.clone(),
                use_id: use_node.clone(),
                name: Sym::new("batch_size"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "uses", Some(span)),
        );
        builder.add_data_flow(
            value_source.clone(),
            value_call_result.clone(),
            DataFlow {
                callable_id: callable.clone(),
                name: Sym::new("batch_size"),
                flow_kind: DataFlowKind::AssignmentValue,
                precision: Sym::new("sg120-exact-data-flow"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "data flow", Some(span)),
        );
        builder.add_parameter_in(
            value_argument.clone(),
            value_formal.clone(),
            ParameterIn {
                call_site_id: call_site.clone(),
                caller_callable_id: callable.clone(),
                callee_callable_id: callable.clone(),
                parameter_name: Sym::new("items"),
                ordinal: 0,
                precision: Sym::new("sg120-exact-parameter-in"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "parameter in", Some(span)),
        );
        builder.add_returns_to(
            value_return.clone(),
            value_call_result.clone(),
            ReturnsTo {
                call_site_id: call_site.clone(),
                caller_callable_id: callable.clone(),
                callee_callable_id: callable.clone(),
                precision: Sym::new("sg120-exact-returns-to"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "returns to", Some(span)),
        );
        builder.add_parameter_out(
            value_formal.clone(),
            value_mutable_state.clone(),
            ParameterOut {
                call_site_id: call_site.clone(),
                caller_callable_id: callable.clone(),
                callee_callable_id: callable.clone(),
                parameter_name: Sym::new("items"),
                ordinal: 0,
                precision: Sym::new("sg120-possible-alias-parameter-out"),
            },
            owner.clone(),
            Some(span),
            Confidence::Unknown,
            schema_evidence(artifact, "parameter out", Some(span)),
        );
        builder.add_throws_to(
            value_exception.clone(),
            cfg_exit.clone(),
            ThrowsTo {
                call_site_id: call_site.clone(),
                caller_callable_id: callable.clone(),
                callee_callable_id: callable.clone(),
                target_kind: ThrowsToTargetKind::CallerExceptionalExit,
                exception_value: Some(Sym::new("ValueError()")),
                exception_type: Some(Sym::new("ValueError")),
                precision: Sym::new("sg120-exact-throws-to"),
            },
            owner.clone(),
            Some(span),
            Confidence::Exact,
            schema_evidence(artifact, "throws to", Some(span)),
        );
        builder.add_decomposes_to(
            DecomposesTo {
                parent_requirement_id: requirement.clone(),
                child_requirement_id: child_requirement.clone(),
            },
            artifact_owner.clone(),
            None,
            Confidence::Exact,
            schema_evidence(artifact, "decomposes to", None),
        );
        builder.add_conditions(
            Conditions {
                condition_requirement_id: condition_requirement.clone(),
                conditioned_requirement_id: child_requirement.clone(),
            },
            artifact_owner.clone(),
            None,
            Confidence::Exact,
            schema_evidence(artifact, "conditions", None),
        );
        builder.add_orders(
            Orders {
                predecessor_requirement_id: requirement.clone(),
                successor_requirement_id: child_requirement.clone(),
                order_key: Sym::new("0001"),
            },
            artifact_owner.clone(),
            None,
            Confidence::Exact,
            schema_evidence(artifact, "orders", None),
        );
        builder.add_traces_to(
            TracesTo {
                requirement_id: requirement.clone(),
                code_fact_id: statement.clone(),
                precision: Sym::new("sg120-exact-trace"),
            },
            artifact_owner.clone(),
            None,
            Confidence::Exact,
            schema_evidence(artifact, "traces to", None),
        );
        builder.add_depends_on_domain_knowledge(
            DependsOnDomainKnowledge {
                requirement_id: requirement.clone(),
                domain_knowledge_id: domain.clone(),
                precision: Sym::new("sg120-stale-domain-knowledge"),
            },
            artifact_owner.clone(),
            None,
            Confidence::Probable,
            schema_evidence(artifact, "depends on domain", None),
        );

        (
            builder.finish(),
            Sg120FixtureIds {
                artifact,
                scope,
                binding,
                callable,
                call_site,
                external,
                statement,
                expression,
                condition,
                symbol,
                definition,
                use_node,
                value_source,
                value_formal,
                value_argument,
                value_return,
                value_call_result,
                value_mutable_state,
                value_exception,
                basic_block,
                domain,
                cfg_entry,
                cfg_exit,
                data_flow_node,
                requirement,
                child_requirement,
                condition_requirement,
                diagnostic,
                contains,
                binds,
                resolves_to,
                calls,
                control_flow,
                controls,
                defines,
                uses,
                data_flow,
                parameter_in,
                returns_to,
                parameter_out,
                throws_to,
                decomposes_to,
                conditions,
                orders,
                traces_to,
                depends_on_domain,
            },
            span,
        )
    }

    fn schema_node(
        node_id: NodeId,
        kind: NodeKind,
        owner: SourceOwnership,
        span: Option<SourceSpan>,
        confidence: Confidence,
        evidence: Vec<Evidence>,
        fact: NodeFact,
    ) -> GraphNode {
        GraphNode {
            node_id,
            fact_id: None,
            payload_hash: None,
            kind,
            owner,
            span,
            confidence,
            uncertainty: Uncertainty::Exact,
            evidence,
            fact,
        }
    }

    fn schema_evidence(
        artifact_id: NodeId,
        summary: &str,
        source_span: Option<SourceSpan>,
    ) -> Vec<Evidence> {
        vec![Evidence {
            kind: EvidenceKind::Parser,
            summary: Sym::from(summary.to_string()),
            source_id: Some(artifact_id),
            source_span,
            content_hash: Some(Sym::new("sha256:sg120")),
            syntax: Some(SyntaxReference {
                kind: Sym::new("sg120-fixture"),
                key_prefix: Some(Sym::from(summary)),
                field_path: Box::new([Sym::new("schema")]),
            }),
        }]
    }

    fn node_fact_kind(fact: &NodeFact) -> NodeKind {
        match fact {
            NodeFact::Artifact(_) => NodeKind::Artifact,
            NodeFact::Scope(_) => NodeKind::Scope,
            NodeFact::Binding(_) => NodeKind::Binding,
            NodeFact::Callable(_) => NodeKind::Callable,
            NodeFact::CallSite(_) => NodeKind::CallSite,
            NodeFact::ExternalTarget(_) => NodeKind::ExternalTarget,
            NodeFact::Statement(_) => NodeKind::Statement,
            NodeFact::Expression(_) => NodeKind::Expression,
            NodeFact::Condition(_) => NodeKind::Condition,
            NodeFact::Symbol(_) => NodeKind::Symbol,
            NodeFact::Definition(_) => NodeKind::Definition,
            NodeFact::Use(_) => NodeKind::Use,
            NodeFact::Value(_) => NodeKind::Value,
            NodeFact::BasicBlock(_) => NodeKind::BasicBlock,
            NodeFact::DomainKnowledge(_) => NodeKind::DomainKnowledge,
            NodeFact::ControlFlow(_) => NodeKind::ControlFlow,
            NodeFact::DataFlow(_) => NodeKind::DataFlow,
            NodeFact::Requirement(_) => NodeKind::Requirement,
            NodeFact::Diagnostic(_) => NodeKind::Diagnostic,
        }
    }

    fn edge_fact_kind(fact: &EdgeFact) -> EdgeKind {
        match fact {
            EdgeFact::Contains(_) => EdgeKind::Contains,
            EdgeFact::Binds(_) => EdgeKind::Binds,
            EdgeFact::ResolvesTo(_) => EdgeKind::ResolvesTo,
            EdgeFact::Calls(_) => EdgeKind::Calls,
            EdgeFact::ControlFlow(_) => EdgeKind::ControlFlow,
            EdgeFact::Controls(_) => EdgeKind::Controls,
            EdgeFact::Defines(_) => EdgeKind::Defines,
            EdgeFact::Uses(_) => EdgeKind::Uses,
            EdgeFact::DataFlow(_) => EdgeKind::DataFlow,
            EdgeFact::ParameterIn(_) => EdgeKind::ParameterIn,
            EdgeFact::ReturnsTo(_) => EdgeKind::ReturnsTo,
            EdgeFact::ParameterOut(_) => EdgeKind::ParameterOut,
            EdgeFact::ThrowsTo(_) => EdgeKind::ThrowsTo,
            EdgeFact::DecomposesTo(_) => EdgeKind::DecomposesTo,
            EdgeFact::Conditions(_) => EdgeKind::Conditions,
            EdgeFact::Orders(_) => EdgeKind::Orders,
            EdgeFact::TracesTo(_) => EdgeKind::TracesTo,
            EdgeFact::DependsOnDomainKnowledge(_) => EdgeKind::DependsOnDomainKnowledge,
        }
    }

    fn graph_with_callable_body(body_span: SourceSpan) -> ProgramSupergraph {
        let artifact_id = test_id("artifact:main");
        let module_scope_id = scope_id(artifact_id, &["module"]);
        let caller_id = callable_id("main.create_incident");
        let callee_id = callable_id("main.calculate_risk");
        let call_site_id = call_site_id(artifact_id, caller_id, "calculate_risk", span(100, 114));
        let requirement_id = stable_id(Tag::Requirement, id_parts![callee_id, "callable-behavior"]);
        let declaration_span = span(10, 25);

        let mut builder = ProgramSupergraphBuilder::new("repo", "python");
        builder.add_artifact(
            Artifact {
                artifact_id: artifact_id.clone(),
                path: Sym::new("main.py"),
                module_path: Sym::new("main"),
                content_hash: None,
            },
            Vec::new(),
        );
        builder.add_scope(
            Scope {
                scope_id: module_scope_id.clone(),
                parent_scope_id: None,
                artifact_id: artifact_id.clone(),
                kind: ScopeKind::Module,
                variant: ScopeVariant::FileModule,
                language_variant: Some(Sym::new("python:module")),
                binding_behavior: ScopeBindingBehavior::Boundary,
                owner_callable_id: None,
                span: None,
            },
            Confidence::Exact,
            Vec::new(),
        );
        for (callable_id, qualified_name, body_span) in [
            (
                caller_id.clone(),
                "main.create_incident",
                Some(span(90, 130)),
            ),
            (callee_id.clone(), "main.calculate_risk", Some(body_span)),
        ] {
            builder.add_callable(
                Callable {
                    callable_id,
                    kind: CallableKind::Function,
                    name: (qualified_name.rsplit('.').next().map(str::to_string)).map(Sym::from),
                    qualified_name: Sym::from(qualified_name.to_string()),
                    artifact_id: artifact_id.clone(),
                    declaration_span,
                    body_span,
                    signature: Signature {
                        parameters: Vec::new(),
                        return_annotation: None,
                    },
                    scope_id: module_scope_id.clone(),
                    attributes: Vec::new(),
                    incoming_local_call_count: 0,
                    external_invocation_metadata: Vec::new(),
                },
                Confidence::Exact,
                Vec::new(),
            );
        }
        builder.add_call_site(
            CallSite {
                call_site_id: call_site_id.clone(),
                artifact_id: artifact_id.clone(),
                enclosing_callable_id: caller_id.clone(),
                span: span(100, 114),
                callee_expression: Sym::new("calculate_risk"),
                argument_shape: ArgumentShape {
                    positional_count: 0,
                    named_arguments: Vec::new(),
                },
                dispatch_kind: DispatchKind::Direct,
                context: crate::ast::CallContext::Body,
            },
            Confidence::Exact,
            Vec::new(),
        );
        builder.add_calls(
            Calls {
                caller_callable_id: caller_id.clone(),
                callee_callable_id: Some(callee_id.clone()),
                external_target_id: None,
                unresolved_target: None,
                call_site_id,
                kind: CallEdgeKind::Direct,
                resolution: Resolution::Exact,
            },
            source_owner(
                Some(artifact_id.clone()),
                Some(module_scope_id),
                Some(caller_id),
            ),
            Some(span(100, 114)),
            Confidence::Exact,
            Vec::new(),
        );

        let mut graph = builder.finish();
        graph.nodes.push(GraphNode {
            node_id: requirement_id.clone(),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::Requirement,
            owner: source_owner(Some(artifact_id.clone()), None, None),
            span: None,
            confidence: Confidence::Probable,
            uncertainty: Uncertainty::Exact,
            evidence: test_evidence("generated callable requirement"),
            fact: NodeFact::Requirement(Requirement {
                requirement_id: requirement_id.clone(),
                kind: RequirementKind::Callable,
                title: Sym::new("Calculate risk behavior"),
                summary: Sym::from("Calculate risk behavior is traceable to the callable subject."
                    .to_string()),
                source_rule: Sym::new("test"),
                path_conditions: Vec::new(),
            }),
        });
        graph.edges.push(GraphEdge {
            edge_id: stable_edge_id(id_parts!["traces-to", requirement_id, callee_id]),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::TracesTo,
            source_id: requirement_id.clone(),
            target_id: Some(callee_id.clone()),
            owner: source_owner(Some(artifact_id), None, None),
            span: None,
            confidence: Confidence::Probable,
            uncertainty: Uncertainty::Exact,
            evidence: test_evidence("requirement traces to callable subject"),
            fact: EdgeFact::TracesTo(TracesTo {
                requirement_id,
                code_fact_id: callee_id,
                precision: Sym::new("subject-reference"),
            }),
        });

        refresh_uncertainty(&mut graph);
        refresh_provenance(&mut graph);
        refresh_fact_identity(&mut graph);
        sort_graph(&mut graph);
        graph.indexes = build_indexes(&graph.nodes, &graph.edges);
        graph
    }

    fn node_by_id<'a>(graph: &'a ProgramSupergraph, node_id: NodeId) -> &'a GraphNode {
        graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .expect("node by stable subject id")
    }

    fn edge_by_id<'a>(graph: &'a ProgramSupergraph, edge_id: EdgeId) -> &'a GraphEdge {
        graph
            .edges
            .iter()
            .find(|edge| edge.edge_id == edge_id)
            .expect("edge by stable subject id")
    }

    fn calls_edge(graph: &ProgramSupergraph) -> &GraphEdge {
        graph
            .edges
            .iter()
            .find(|edge| matches!(edge.fact, EdgeFact::Calls(_)))
            .expect("calls edge")
    }

    fn node_uncertainty_by_id(graph: &ProgramSupergraph, node_id: NodeId) -> Option<Uncertainty> {
        graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.uncertainty)
    }

    fn edge_uncertainty_by_id(graph: &ProgramSupergraph, edge_id: EdgeId) -> Option<Uncertainty> {
        graph
            .edges
            .iter()
            .find(|edge| edge.edge_id == edge_id)
            .map(|edge| edge.uncertainty)
    }

    fn test_evidence(summary: &str) -> Vec<Evidence> {
        vec![Evidence {
            kind: EvidenceKind::Inference,
            summary: Sym::from(summary.to_string()),
            source_id: None,
            source_span: None,
            content_hash: None,
            syntax: None,
        }]
    }
}

