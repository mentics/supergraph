use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::ast::{CallContext, SourceSpan};

pub const SCHEMA_VERSION: &str = "program-supergraph.v2";
pub const DOMAIN_KNOWLEDGE_RECORD_SCHEMA_VERSION: &str = "domain-knowledge-record.v1";

pub type StableId = String;
pub type SubjectId = StableId;
pub type FactId = StableId;
pub type NodeId = StableId;
pub type EdgeId = StableId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramSupergraph {
    pub schema_version: String,
    pub language: String,
    pub root: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub indexes: GraphIndexes,
    #[serde(skip)]
    pub(crate) id_cache: IdCache,
}

/// Lazily built membership sets that make `insert_node`/`insert_edge` O(1).
///
/// A set is trusted only while its recorded length matches the backing vector;
/// any other mutation of `nodes`/`edges` changes the length (or calls
/// `invalidate`) and triggers a rebuild on the next insert.
#[derive(Debug, Clone, Default)]
pub(crate) struct IdCache {
    node_len: usize,
    edge_len: usize,
    node_ids: HashSet<NodeId>,
    edge_ids: HashSet<EdgeId>,
}

impl PartialEq for IdCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for IdCache {}

impl ProgramSupergraph {
    /// Writes the compact JSON form, byte-identical to `serde_json::to_writer(self)`, but
    /// serializes nodes, edges and indexes on separate threads.
    pub fn write_json<W: std::io::Write>(&self, out: &mut W) -> std::io::Result<()> {
        fn to_error(error: serde_json::Error) -> std::io::Error {
            std::io::Error::other(error)
        }
        fn serialize_chunk<T: Serialize>(items: &[T]) -> Result<Vec<u8>, serde_json::Error> {
            let mut buffer = Vec::with_capacity(items.len() * 1024);
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    buffer.push(b',');
                }
                serde_json::to_writer(&mut buffer, item)?;
            }
            Ok(buffer)
        }

        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let node_chunk = self.nodes.len().div_ceil(threads).max(1024);
        let edge_chunk = self.edges.len().div_ceil(threads).max(1024);
        std::thread::scope(|scope| {
            let nodes = self
                .nodes
                .chunks(node_chunk)
                .map(|chunk| scope.spawn(move || serialize_chunk(chunk)))
                .collect::<Vec<_>>();
            let edges = self
                .edges
                .chunks(edge_chunk)
                .map(|chunk| scope.spawn(move || serialize_chunk(chunk)))
                .collect::<Vec<_>>();
            let indexes = scope.spawn(|| serde_json::to_vec(&self.indexes));

            out.write_all(b"{\"schema_version\":")?;
            serde_json::to_writer(&mut *out, &self.schema_version)?;
            out.write_all(b",\"language\":")?;
            serde_json::to_writer(&mut *out, &self.language)?;
            out.write_all(b",\"root\":")?;
            serde_json::to_writer(&mut *out, &self.root)?;
            for (name, parts) in [("nodes", nodes), ("edges", edges)] {
                out.write_all(b",\"")?;
                out.write_all(name.as_bytes())?;
                out.write_all(b"\":[")?;
                for (position, part) in parts.into_iter().enumerate() {
                    let bytes = part.join().expect("serializer thread").map_err(to_error)?;
                    if position > 0 && !bytes.is_empty() {
                        out.write_all(b",")?;
                    }
                    out.write_all(&bytes)?;
                }
                out.write_all(b"]")?;
            }
            out.write_all(b",\"indexes\":")?;
            let indexes = indexes.join().expect("serializer thread").map_err(to_error)?;
            out.write_all(&indexes)?;
            out.write_all(b"}")
        })
    }

    pub(crate) fn invalidate_id_cache(&mut self) {
        self.id_cache = IdCache::default();
    }

    /// Pushes `node` unless a node with the same id exists. Returns whether it was added.
    pub(crate) fn push_node_if_new(&mut self, node: GraphNode) -> bool {
        let cache = &mut self.id_cache;
        if cache.node_len != self.nodes.len() || cache.node_ids.len() != self.nodes.len() {
            cache.node_ids = self.nodes.iter().map(|node| node.node_id.clone()).collect();
        }
        if !cache.node_ids.insert(node.node_id.clone()) {
            cache.node_len = self.nodes.len();
            return false;
        }
        self.nodes.push(node);
        cache.node_len = self.nodes.len();
        true
    }

    /// Pushes `edge` unless an edge with the same id exists. Returns whether it was added.
    pub(crate) fn push_edge_if_new(&mut self, edge: GraphEdge) -> bool {
        let cache = &mut self.id_cache;
        if cache.edge_len != self.edges.len() || cache.edge_ids.len() != self.edges.len() {
            cache.edge_ids = self.edges.iter().map(|edge| edge.edge_id.clone()).collect();
        }
        if !cache.edge_ids.insert(edge.edge_id.clone()) {
            cache.edge_len = self.edges.len();
            return false;
        }
        self.edges.push(edge);
        cache.edge_len = self.edges.len();
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub node_id: NodeId,
    #[serde(default)]
    pub fact_id: FactId,
    #[serde(default)]
    pub payload_hash: String,
    pub kind: NodeKind,
    pub owner: SourceOwnership,
    pub span: Option<SourceSpan>,
    pub confidence: Confidence,
    #[serde(default)]
    pub uncertainty: Uncertainty,
    pub evidence: Vec<Evidence>,
    pub fact: NodeFact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub edge_id: EdgeId,
    #[serde(default)]
    pub fact_id: FactId,
    #[serde(default)]
    pub payload_hash: String,
    pub kind: EdgeKind,
    pub source_id: NodeId,
    pub target_id: Option<NodeId>,
    pub owner: SourceOwnership,
    pub span: Option<SourceSpan>,
    pub confidence: Confidence,
    #[serde(default)]
    pub uncertainty: Uncertainty,
    pub evidence: Vec<Evidence>,
    pub fact: EdgeFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeKind {
    Artifact,
    Scope,
    Binding,
    Callable,
    CallSite,
    ExternalTarget,
    Statement,
    Expression,
    Condition,
    Symbol,
    Definition,
    Use,
    Value,
    BasicBlock,
    DomainKnowledge,
    ControlFlow,
    DataFlow,
    Requirement,
    Diagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EdgeKind {
    Contains,
    Binds,
    ResolvesTo,
    Calls,
    ControlFlow,
    Controls,
    Defines,
    Uses,
    DataFlow,
    ParameterIn,
    ReturnsTo,
    ParameterOut,
    ThrowsTo,
    DecomposesTo,
    Conditions,
    Orders,
    TracesTo,
    DependsOnDomainKnowledge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeFact {
    Artifact(Artifact),
    Scope(Scope),
    Binding(Binding),
    Callable(Callable),
    CallSite(CallSite),
    ExternalTarget(ExternalTarget),
    Statement(Statement),
    Expression(Expression),
    Condition(Condition),
    Symbol(Symbol),
    Definition(Definition),
    Use(Use),
    Value(Value),
    BasicBlock(BasicBlock),
    DomainKnowledge(DomainKnowledge),
    ControlFlow(ControlFlowNode),
    DataFlow(DataFlowNode),
    Requirement(Requirement),
    Diagnostic(Diagnostic),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeFact {
    Contains(Contains),
    Binds(Binds),
    ResolvesTo(ResolvesTo),
    Calls(Calls),
    ControlFlow(ControlFlow),
    Controls(Controls),
    Defines(Defines),
    Uses(Uses),
    DataFlow(DataFlow),
    ParameterIn(ParameterIn),
    ReturnsTo(ReturnsTo),
    ParameterOut(ParameterOut),
    ThrowsTo(ThrowsTo),
    DecomposesTo(DecomposesTo),
    Conditions(Conditions),
    Orders(Orders),
    TracesTo(TracesTo),
    DependsOnDomainKnowledge(DependsOnDomainKnowledge),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOwnership {
    pub artifact_id: Option<NodeId>,
    pub scope_id: Option<NodeId>,
    pub callable_id: Option<NodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    Unknown,
    Probable,
    Exact,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Uncertainty {
    #[default]
    Exact,
    Probable,
    Possible,
    External,
    Unresolved,
    Ambiguous,
    Stale,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub summary: String,
    pub source_id: Option<NodeId>,
    pub source_span: Option<SourceSpan>,
    pub content_hash: Option<String>,
    pub syntax: Option<SyntaxReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EvidenceKind {
    Parser,
    Resolver,
    Adapter,
    Inference,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyntaxReference {
    pub kind: String,
    pub node_key: Option<String>,
    pub field_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: NodeId,
    pub path: String,
    pub module_path: String,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub scope_id: NodeId,
    pub parent_scope_id: Option<NodeId>,
    pub artifact_id: NodeId,
    pub kind: ScopeKind,
    #[serde(default)]
    pub variant: ScopeVariant,
    #[serde(default)]
    pub language_variant: Option<String>,
    #[serde(default)]
    pub binding_behavior: ScopeBindingBehavior,
    pub owner_callable_id: Option<NodeId>,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScopeKind {
    Module,
    Class,
    Function,
    Block,
    Catch,
    Comprehension,
    LanguageSpecific,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScopeVariant {
    #[default]
    Unknown,
    FileModule,
    ClassBody,
    FunctionBody,
    StatementBlock,
    CatchHandler,
    Comprehension,
    LanguageSpecific,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScopeBindingBehavior {
    #[default]
    Boundary,
    Transparent,
    LanguageSpecific,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub binding_id: NodeId,
    pub scope_id: NodeId,
    pub name: String,
    pub kind: BindingKind,
    pub target: BindingTarget,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BindingKind {
    Import,
    Assignment,
    Parameter,
    Class,
    Function,
    Method,
    Field,
    External,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BindingTarget {
    Callable(NodeId),
    Class(NodeId),
    Module(NodeId),
    External(NodeId),
    Value(String),
    Unresolved(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callable {
    pub callable_id: NodeId,
    pub kind: CallableKind,
    pub name: Option<String>,
    pub qualified_name: String,
    pub artifact_id: NodeId,
    pub declaration_span: SourceSpan,
    pub body_span: Option<SourceSpan>,
    pub signature: Signature,
    pub scope_id: NodeId,
    pub attributes: Vec<String>,
    pub incoming_local_call_count: usize,
    pub external_invocation_metadata: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CallableKind {
    Function,
    Method,
    Constructor,
    ModuleInitializer,
    UnknownCallable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub parameters: Vec<String>,
    pub return_annotation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSite {
    pub call_site_id: NodeId,
    pub artifact_id: NodeId,
    pub enclosing_callable_id: NodeId,
    pub span: SourceSpan,
    pub callee_expression: String,
    pub argument_shape: ArgumentShape,
    pub dispatch_kind: DispatchKind,
    pub context: CallContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgumentShape {
    pub positional_count: usize,
    pub named_arguments: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DispatchKind {
    Direct,
    Method,
    Constructor,
    Decorator,
    HigherOrder,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalTarget {
    pub external_target_id: NodeId,
    pub ecosystem: String,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub module_path: Option<String>,
    pub qualified_name: String,
    pub member_path: Option<String>,
    pub target_kind: ExternalTargetKind,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub statement_id: NodeId,
    pub callable_id: NodeId,
    pub parent_statement_id: Option<NodeId>,
    pub kind: StatementKind,
    pub ordinal: usize,
    pub child_statement_ids: Vec<NodeId>,
    pub expression_ids: Vec<NodeId>,
    #[serde(default)]
    pub control_effects: Vec<StatementControlEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StatementKind {
    Declaration,
    Assignment,
    Expression,
    Branch,
    Loop,
    Return,
    Raise,
    Throw,
    Function,
    Class,
    Break,
    Continue,
    Try,
    Catch,
    Finally,
    Import,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementControlEffect {
    pub kind: StatementControlEffectKind,
    pub target_statement_id: Option<NodeId>,
    pub fallthrough: FallthroughBehavior,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StatementControlEffectKind {
    NormalFallthrough,
    Break,
    Continue,
    Return,
    Raise,
    Throw,
    TryEnter,
    CatchEnter,
    FinallyEnter,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expression {
    pub expression_id: NodeId,
    pub callable_id: NodeId,
    pub statement_id: Option<NodeId>,
    pub parent_expression_id: Option<NodeId>,
    pub kind: ExpressionKind,
    pub ordinal: usize,
    pub child_expression_ids: Vec<NodeId>,
    pub symbol_id: Option<NodeId>,
    pub value_id: Option<NodeId>,
    pub original_text: Option<String>,
    pub normalized: NormalizedExpression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ExpressionKind {
    Identifier,
    Literal,
    Call,
    FieldAccess,
    IndexAccess,
    Assignment,
    UnaryOperator,
    BinaryOperator,
    Conditional,
    Await,
    Yield,
    Lambda,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedExpression {
    pub canonical: Option<String>,
    pub operator: Option<String>,
    pub identifier: Option<String>,
    pub member: Option<String>,
    pub literal: Option<ValueLiteral>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub condition_id: NodeId,
    pub callable_id: NodeId,
    #[serde(default)]
    pub statement_id: Option<NodeId>,
    pub expression_id: Option<NodeId>,
    pub kind: ConditionKind,
    pub controlled_statement_ids: Vec<NodeId>,
    pub outcome_labels: Vec<String>,
    #[serde(default)]
    pub regions: Vec<ControlRegion>,
    #[serde(default)]
    pub continuation: Option<ContinuationPoint>,
    #[serde(default)]
    pub fallthrough: FallthroughBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConditionKind {
    Branch,
    Loop,
    Guard,
    MatchArm,
    CatchFilter,
    ExceptionRegion,
    ShortCircuit,
    ConditionalExpression,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRegion {
    pub kind: ControlRegionKind,
    pub label: String,
    pub statement_ids: Vec<NodeId>,
    pub entry_statement_id: Option<NodeId>,
    pub exit_statement_id: Option<NodeId>,
    pub fallthrough: FallthroughBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ControlRegionKind {
    BranchBody,
    ElseBody,
    LoopBody,
    LoopContinuation,
    TryBody,
    CatchBody,
    FinallyBody,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationPoint {
    pub kind: ContinuationKind,
    pub target_statement_id: Option<NodeId>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ContinuationKind {
    NextStatement,
    LoopCondition,
    LoopUpdate,
    LoopExit,
    CallableExit,
    ExceptionHandler,
    ExceptionalExit,
    Finally,
    Fallthrough,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FallthroughBehavior {
    #[default]
    Unknown,
    FallsThrough,
    DoesNotFallThrough,
    Conditional,
    LanguageSpecific,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub symbol_id: NodeId,
    pub scope_id: NodeId,
    pub name: String,
    pub kind: SymbolKind,
    pub binding_id: Option<NodeId>,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SymbolKind {
    Local,
    Parameter,
    Import,
    Export,
    Field,
    Property,
    Callable,
    Type,
    External,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub definition_id: NodeId,
    pub callable_id: NodeId,
    pub symbol_id: Option<NodeId>,
    pub value_id: Option<NodeId>,
    pub kind: DefinitionKind,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DefinitionKind {
    Declaration,
    Assignment,
    Parameter,
    Import,
    FieldWrite,
    ReturnValue,
    ExceptionValue,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Use {
    pub use_id: NodeId,
    pub callable_id: NodeId,
    pub symbol_id: Option<NodeId>,
    pub value_id: Option<NodeId>,
    pub kind: UseKind,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UseKind {
    Read,
    CallCallee,
    CallArgument,
    Condition,
    Return,
    FieldRead,
    IndexRead,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub value_id: NodeId,
    pub callable_id: Option<NodeId>,
    pub kind: ValueKind,
    #[serde(default)]
    pub role: ValueRole,
    pub symbol_id: Option<NodeId>,
    pub expression_id: Option<NodeId>,
    #[serde(default)]
    pub call_site_id: Option<NodeId>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ordinal: Option<usize>,
    #[serde(default)]
    pub state_of_value_id: Option<NodeId>,
    pub type_hint: Option<String>,
    pub literal: Option<ValueLiteral>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ValueKind {
    Literal,
    ComputedExpression,
    Object,
    Field,
    Index,
    Parameter,
    Argument,
    CallResult,
    Return,
    Exception,
    Merge,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ValueRole {
    #[default]
    Unknown,
    FormalParameter,
    Receiver,
    Argument,
    ReturnValue,
    CallResult,
    ExceptionalValue,
    MutableArgumentState,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ValueLiteral {
    Null,
    Boolean(bool),
    Integer(String),
    Float(String),
    String(String),
    Bytes(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub basic_block_id: NodeId,
    pub callable_id: NodeId,
    pub kind: BasicBlockKind,
    pub ordinal: usize,
    pub statement_ids: Vec<NodeId>,
    pub entry_node_id: Option<NodeId>,
    pub exit_node_id: Option<NodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BasicBlockKind {
    Entry,
    StraightLine,
    BranchArm,
    LoopBody,
    Handler,
    Exit,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainKnowledge {
    pub domain_knowledge_id: NodeId,
    pub scope: DomainKnowledgeScope,
    pub summary: String,
    pub source: DomainKnowledgeSource,
    pub applies_to: Vec<NodeId>,
    pub status: DomainKnowledgeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DomainKnowledgeScope {
    Repository,
    Artifact(NodeId),
    Callable(NodeId),
    Requirement(NodeId),
    External(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DomainKnowledgeSource {
    User,
    Documentation,
    Inference,
    ExternalReference(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DomainKnowledgeStatus {
    Active,
    Stale,
    Superseded,
    Unknown,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeRecord {
    #[serde(default = "domain_knowledge_record_schema_version")]
    pub record_schema_version: String,
    pub record_id: StableId,
    #[serde(default = "unknown_domain_knowledge_status")]
    pub status: DomainKnowledgeStatus,
    pub selector: DomainKnowledgeSelector,
    pub label: DomainKnowledgeLabel,
    #[serde(default)]
    pub grouping_hints: DomainKnowledgeGroupingHints,
    #[serde(default)]
    pub rendering_hints: DomainKnowledgeRenderingHints,
    #[serde(default)]
    pub provenance: DomainKnowledgeRecordProvenance,
    #[serde(default = "default_compatible_supergraph_schema_versions")]
    pub compatible_supergraph_schema_versions: Vec<String>,
    #[serde(default)]
    pub last_seen_evidence_fingerprints: Vec<DomainKnowledgeEvidenceFingerprint>,
    #[serde(default)]
    pub accepted_representative_examples: Vec<DomainKnowledgeRepresentativeExample>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeSelector {
    pub selector_id: StableId,
    pub description: String,
    #[serde(default)]
    pub predicates: Vec<DomainKnowledgeSelectorPredicate>,
    #[serde(default)]
    pub expected_match: DomainKnowledgeMatchExpectation,
    #[serde(default)]
    pub expected_fact_kinds: Vec<NodeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DomainKnowledgeSelectorPredicate {
    /// Match graph nodes by deterministic supergraph node kind.
    FactKind(NodeKind),
    /// Match graph edges by deterministic supergraph edge kind.
    EdgeFactKind(EdgeKind),
    FieldEquals {
        path: String,
        value: String,
    },
    FieldContains {
        path: String,
        value: String,
    },
    HasField {
        path: String,
    },
    /// Match a field or property name present on the selected fact payload.
    PropertyExists {
        owner_path: String,
        property_name: String,
    },
    Relationship {
        edge_kind: EdgeKind,
        target_kind: Option<NodeKind>,
    },
    /// Match a deterministic relationship with optional source and target kinds.
    SourceTargetRelationship {
        edge_kind: EdgeKind,
        source_kind: Option<NodeKind>,
        target_kind: Option<NodeKind>,
    },
    /// Match a bounded local call path by declarative call-target patterns.
    CallPath {
        path: DomainKnowledgeCallPathSelector,
    },
    /// Match call targets in source order within a local callable.
    OrderedCalls {
        pattern: DomainKnowledgeOrderedCallPattern,
    },
    EvidenceFingerprint {
        fingerprint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeCallPathSelector {
    #[serde(default)]
    pub start_kind: Option<NodeKind>,
    pub steps: Vec<DomainKnowledgeCallPathStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeCallPathStep {
    #[serde(default)]
    pub call_kind: Option<CallEdgeKind>,
    pub target: DomainKnowledgeCallTargetPattern,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeCallTargetPattern {
    #[serde(default)]
    pub node_kind: Option<NodeKind>,
    #[serde(default)]
    pub qualified_name: Option<String>,
    #[serde(default)]
    pub name_contains: Option<String>,
    #[serde(default)]
    pub external_package: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeOrderedCallPattern {
    pub calls: Vec<DomainKnowledgeCallPattern>,
    #[serde(default)]
    pub allow_intervening_calls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeCallPattern {
    pub target: DomainKnowledgeCallTargetPattern,
    #[serde(default)]
    pub argument_contains: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DomainKnowledgeMatchExpectation {
    #[default]
    Any,
    One,
    Many,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeLabel {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeGroupingHints {
    #[serde(default)]
    pub group_key: Option<String>,
    #[serde(default)]
    pub group_label: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeRenderingHints {
    #[serde(default)]
    pub title_template: Option<String>,
    #[serde(default)]
    pub summary_template: Option<String>,
    #[serde(default)]
    pub terminology: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeRecordProvenance {
    #[serde(default = "user_domain_knowledge_source")]
    pub source: DomainKnowledgeSource,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub reviewed_by: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Default for DomainKnowledgeRecordProvenance {
    fn default() -> Self {
        Self {
            source: DomainKnowledgeSource::User,
            created_by: None,
            reviewed_by: None,
            created_at: None,
            updated_at: None,
            notes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeEvidenceFingerprint {
    pub fingerprint: String,
    #[serde(default)]
    pub fact_kind: Option<NodeKind>,
    #[serde(default)]
    pub match_count: usize,
    #[serde(default)]
    pub supergraph_schema_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainKnowledgeRepresentativeExample {
    pub subject_id: StableId,
    pub evidence_fingerprint: String,
    #[serde(default)]
    pub fact_kind: Option<NodeKind>,
    #[serde(default)]
    pub label: Option<String>,
}

pub fn migrate_domain_knowledge_record(mut record: DomainKnowledgeRecord) -> DomainKnowledgeRecord {
    if record.record_schema_version.is_empty() {
        record.record_schema_version = DOMAIN_KNOWLEDGE_RECORD_SCHEMA_VERSION.to_string();
    }
    if record.compatible_supergraph_schema_versions.is_empty() {
        record.compatible_supergraph_schema_versions =
            default_compatible_supergraph_schema_versions();
    }
    record
}

fn domain_knowledge_record_schema_version() -> String {
    DOMAIN_KNOWLEDGE_RECORD_SCHEMA_VERSION.to_string()
}

fn default_compatible_supergraph_schema_versions() -> Vec<String> {
    vec![SCHEMA_VERSION.to_string()]
}

fn unknown_domain_knowledge_status() -> DomainKnowledgeStatus {
    DomainKnowledgeStatus::Unknown
}

fn user_domain_knowledge_source() -> DomainKnowledgeSource {
    DomainKnowledgeSource::User
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlFlowNode {
    pub cfg_node_id: NodeId,
    pub callable_id: NodeId,
    pub role: ControlFlowNodeRole,
    pub label: String,
    pub semantic_kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ControlFlowNodeRole {
    Entry,
    Exit,
    Statement,
    Condition,
    Merge,
    Return,
    Raise,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFlowNode {
    pub data_flow_node_id: NodeId,
    pub callable_id: NodeId,
    pub role: DataFlowNodeRole,
    pub name: Option<String>,
    pub text: String,
    pub semantic_kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DataFlowNodeRole {
    Definition,
    Use,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    pub requirement_id: NodeId,
    pub kind: RequirementKind,
    pub title: String,
    pub summary: String,
    pub source_rule: String,
    #[serde(default)]
    pub path_conditions: Vec<PathConditionSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RequirementKind {
    Boundary,
    Callable,
    Condition,
    Return,
    Raise,
    Call,
    Definition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathConditionSummary {
    pub controlling_cfg_node_id: NodeId,
    #[serde(default)]
    pub condition_id: Option<NodeId>,
    #[serde(default)]
    pub expression_id: Option<NodeId>,
    pub outcome: ControlFlowOutcome,
    #[serde(default)]
    pub branch_arm: Option<ControlFlowBranchArm>,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ExternalTargetKind {
    Function,
    Method,
    Constructor,
    Decorator,
    ClassCallable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub diagnostic_id: NodeId,
    pub kind: DiagnosticKind,
    pub severity: Severity,
    pub message: String,
    pub artifact_id: Option<NodeId>,
    pub span: Option<SourceSpan>,
    pub related: Vec<NodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DiagnosticKind {
    UnresolvedImport,
    UnresolvedSymbol,
    ExternalTargetUnknownPackage,
    AmbiguousSymbol,
    DynamicDispatch,
    UnsupportedSyntax,
    ParseError,
    AliasUncertainty,
    StaleDomainKnowledge,
    MissingDomainKnowledge,
    UnreachableStatement,
    UntraceableRequirement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contains {
    pub container_id: NodeId,
    pub member_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binds {
    pub scope_id: NodeId,
    pub binding_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvesTo {
    pub binding_id: NodeId,
    pub target_id: NodeId,
    #[serde(default)]
    pub resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calls {
    pub caller_callable_id: NodeId,
    pub callee_callable_id: Option<NodeId>,
    pub external_target_id: Option<NodeId>,
    pub unresolved_target: Option<String>,
    pub call_site_id: NodeId,
    pub kind: CallEdgeKind,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlFlow {
    pub callable_id: NodeId,
    pub flow_kind: ControlFlowKind,
    #[serde(default)]
    pub outcome: ControlFlowOutcome,
    #[serde(default)]
    pub branch_arm: Option<ControlFlowBranchArm>,
    pub precision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ControlFlowKind {
    Entry,
    Exit,
    Sequential,
    Branch,
    LoopBack,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ControlFlowOutcome {
    #[default]
    Unknown,
    Entry,
    Exit,
    True,
    False,
    Arm,
    Fallthrough,
    Exception,
    Finally,
    Break,
    Continue,
    LoopBack,
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlFlowBranchArm {
    pub label: String,
    pub ordinal: usize,
    pub region_kind: Option<ControlRegionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controls {
    pub callable_id: NodeId,
    pub condition_id: NodeId,
    pub controlled_id: NodeId,
    pub precision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defines {
    pub callable_id: NodeId,
    pub definition_id: NodeId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    pub callable_id: NodeId,
    pub use_id: NodeId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFlow {
    pub callable_id: NodeId,
    pub name: String,
    pub flow_kind: DataFlowKind,
    pub precision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DataFlowKind {
    DefinitionToUse,
    DefinitionToReturn,
    CallArgument,
    ExpressionOperand,
    AssignmentValue,
    ReturnValue,
    FieldAccess,
    IndexAccess,
    MergeValue,
    LoopCarriedValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterIn {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    pub parameter_name: String,
    pub ordinal: usize,
    pub precision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnsTo {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    pub precision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterOut {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    pub parameter_name: String,
    pub ordinal: usize,
    pub precision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThrowsTo {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    #[serde(default)]
    pub target_kind: ThrowsToTargetKind,
    #[serde(default)]
    pub exception_value: Option<String>,
    #[serde(default)]
    pub exception_type: Option<String>,
    pub precision: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ThrowsToTargetKind {
    #[default]
    CallExceptionalValue,
    Handler,
    CallerExceptionalExit,
    CalleeExceptionalExit,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecomposesTo {
    pub parent_requirement_id: NodeId,
    pub child_requirement_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conditions {
    pub condition_requirement_id: NodeId,
    pub conditioned_requirement_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Orders {
    pub predecessor_requirement_id: NodeId,
    pub successor_requirement_id: NodeId,
    pub order_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracesTo {
    pub requirement_id: NodeId,
    pub code_fact_id: NodeId,
    pub precision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependsOnDomainKnowledge {
    pub requirement_id: NodeId,
    pub domain_knowledge_id: NodeId,
    pub precision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CallEdgeKind {
    Direct,
    Method,
    Constructor,
    Decorator,
    External,
    PossibleDynamic,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    Exact,
    Probable,
    Possible,
    External,
    Unresolved,
    Ambiguous,
    Unsupported,
}

pub fn uncertainty_from_confidence(confidence: &Confidence) -> Uncertainty {
    match confidence {
        Confidence::Exact => Uncertainty::Exact,
        Confidence::Probable => Uncertainty::Probable,
        Confidence::Unknown => Uncertainty::Possible,
    }
}

pub fn uncertainty_from_resolution(resolution: Resolution) -> Uncertainty {
    match resolution {
        Resolution::Exact => Uncertainty::Exact,
        Resolution::Probable => Uncertainty::Probable,
        Resolution::Possible => Uncertainty::Possible,
        Resolution::External => Uncertainty::External,
        Resolution::Unresolved => Uncertainty::Unresolved,
        Resolution::Ambiguous => Uncertainty::Ambiguous,
        Resolution::Unsupported => Uncertainty::Unsupported,
    }
}

pub fn uncertainty_from_diagnostic_kind(kind: DiagnosticKind) -> Uncertainty {
    match kind {
        DiagnosticKind::UnresolvedImport | DiagnosticKind::UnresolvedSymbol => {
            Uncertainty::Unresolved
        }
        DiagnosticKind::ExternalTargetUnknownPackage => Uncertainty::External,
        DiagnosticKind::AmbiguousSymbol => Uncertainty::Ambiguous,
        DiagnosticKind::DynamicDispatch => Uncertainty::Possible,
        DiagnosticKind::UnsupportedSyntax | DiagnosticKind::ParseError => Uncertainty::Unsupported,
        DiagnosticKind::StaleDomainKnowledge => Uncertainty::Stale,
        DiagnosticKind::MissingDomainKnowledge => Uncertainty::Unresolved,
        DiagnosticKind::AliasUncertainty => Uncertainty::Possible,
        DiagnosticKind::UnreachableStatement => Uncertainty::Exact,
        DiagnosticKind::UntraceableRequirement => Uncertainty::Unsupported,
    }
}

pub fn uncertainty_from_domain_knowledge_status(status: DomainKnowledgeStatus) -> Uncertainty {
    match status {
        DomainKnowledgeStatus::Active => Uncertainty::Probable,
        DomainKnowledgeStatus::Stale => Uncertainty::Stale,
        DomainKnowledgeStatus::Superseded => Uncertainty::Stale,
        DomainKnowledgeStatus::Unknown => Uncertainty::Possible,
        DomainKnowledgeStatus::Missing => Uncertainty::Unresolved,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceSpanIndexKey {
    pub artifact_id: Option<NodeId>,
    pub span: SourceSpan,
}

impl Serialize for SourceSpanIndexKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!(
            "{}\u{1f}{}:{}:{}:{}:{}:{}",
            self.artifact_id.as_deref().unwrap_or(""),
            self.span.start_byte,
            self.span.end_byte,
            self.span.start_row,
            self.span.start_column,
            self.span.end_row,
            self.span.end_column
        ))
    }
}

impl<'de> Deserialize<'de> for SourceSpanIndexKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let (artifact, span) = value
            .split_once('\u{1f}')
            .ok_or_else(|| de::Error::custom("invalid source span index key"))?;
        let parts = span.split(':').collect::<Vec<_>>();
        if parts.len() != 6 {
            return Err(de::Error::custom("invalid source span index key span"));
        }

        let parse = |part: &str| {
            part.parse::<usize>()
                .map_err(|_| de::Error::custom("invalid source span index key number"))
        };

        Ok(Self {
            artifact_id: (!artifact.is_empty()).then(|| artifact.to_string()),
            span: SourceSpan {
                start_byte: parse(parts[0])?,
                end_byte: parse(parts[1])?,
                start_row: parse(parts[2])?,
                start_column: parse(parts[3])?,
                end_row: parse(parts[4])?,
                end_column: parse(parts[5])?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GraphIndexes {
    pub node_position_by_id: BTreeMap<NodeId, usize>,
    pub edge_position_by_id: BTreeMap<EdgeId, usize>,
    pub nodes_by_kind: BTreeMap<NodeKind, Vec<NodeId>>,
    pub edges_by_kind: BTreeMap<EdgeKind, Vec<EdgeId>>,
    pub nodes_by_uncertainty: BTreeMap<Uncertainty, Vec<NodeId>>,
    pub edges_by_uncertainty: BTreeMap<Uncertainty, Vec<EdgeId>>,
    pub outgoing_edges_by_node: BTreeMap<NodeId, Vec<EdgeId>>,
    pub incoming_edges_by_node: BTreeMap<NodeId, Vec<EdgeId>>,
    pub outgoing_edges_by_node_and_kind: BTreeMap<NodeId, BTreeMap<EdgeKind, Vec<EdgeId>>>,
    pub incoming_edges_by_node_and_kind: BTreeMap<NodeId, BTreeMap<EdgeKind, Vec<EdgeId>>>,
    pub source_span_to_nodes: BTreeMap<SourceSpanIndexKey, Vec<NodeId>>,
    pub artifact_to_nodes: BTreeMap<NodeId, Vec<NodeId>>,
    pub callable_to_nodes: BTreeMap<NodeId, Vec<NodeId>>,
    pub owner_to_nodes: BTreeMap<NodeId, Vec<NodeId>>,
    pub owner_to_edges: BTreeMap<NodeId, Vec<EdgeId>>,
    pub symbol_to_definitions: BTreeMap<NodeId, Vec<NodeId>>,
    pub symbol_to_uses: BTreeMap<NodeId, Vec<NodeId>>,
    pub requirement_to_code: BTreeMap<NodeId, Vec<NodeId>>,
    pub code_to_requirements: BTreeMap<NodeId, Vec<NodeId>>,
    #[serde(default)]
    pub requirement_to_domain_knowledge: BTreeMap<NodeId, Vec<NodeId>>,
    #[serde(default)]
    pub domain_knowledge_to_requirements: BTreeMap<NodeId, Vec<NodeId>>,
    pub calls_by_caller: BTreeMap<NodeId, Vec<EdgeId>>,
    pub calls_by_concrete_target: BTreeMap<NodeId, Vec<EdgeId>>,
    pub call_site_to_calls: BTreeMap<NodeId, Vec<EdgeId>>,
    pub caller_to_concrete_target_calls: BTreeMap<NodeId, BTreeMap<NodeId, Vec<EdgeId>>>,
    pub caller_to_concrete_call_targets: BTreeMap<NodeId, Vec<NodeId>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_domain_knowledge_record(record_id: &str) -> DomainKnowledgeRecord {
        DomainKnowledgeRecord {
            record_schema_version: DOMAIN_KNOWLEDGE_RECORD_SCHEMA_VERSION.to_string(),
            record_id: record_id.to_string(),
            status: DomainKnowledgeStatus::Active,
            selector: DomainKnowledgeSelector {
                selector_id: "selector:incident-endpoint".to_string(),
                description: "incident triage endpoint handlers".to_string(),
                predicates: vec![
                    DomainKnowledgeSelectorPredicate::FactKind(NodeKind::Callable),
                    DomainKnowledgeSelectorPredicate::FieldContains {
                        path: "qualified_name".to_string(),
                        value: "incident".to_string(),
                    },
                ],
                expected_match: DomainKnowledgeMatchExpectation::Many,
                expected_fact_kinds: vec![NodeKind::Callable],
            },
            label: DomainKnowledgeLabel {
                name: "Incident triage".to_string(),
                summary: "User-facing incident triage workflow".to_string(),
                aliases: vec!["triage".to_string()],
            },
            grouping_hints: DomainKnowledgeGroupingHints {
                group_key: Some("incident".to_string()),
                group_label: Some("Incident workflows".to_string()),
                priority: Some(10),
            },
            rendering_hints: DomainKnowledgeRenderingHints {
                title_template: Some("{label} behavior".to_string()),
                summary_template: Some("{label}: {mechanical_summary}".to_string()),
                terminology: BTreeMap::from([("handler".to_string(), "workflow step".to_string())]),
            },
            provenance: DomainKnowledgeRecordProvenance {
                source: DomainKnowledgeSource::Documentation,
                created_by: Some("agent".to_string()),
                reviewed_by: Some("user".to_string()),
                created_at: Some("2026-06-24T00:00:00Z".to_string()),
                updated_at: None,
                notes: vec!["approved project terminology".to_string()],
            },
            compatible_supergraph_schema_versions: vec![SCHEMA_VERSION.to_string()],
            last_seen_evidence_fingerprints: vec![DomainKnowledgeEvidenceFingerprint {
                fingerprint: "evidence-shape:incident-callable".to_string(),
                fact_kind: Some(NodeKind::Callable),
                match_count: 3,
                supergraph_schema_version: Some(SCHEMA_VERSION.to_string()),
            }],
            accepted_representative_examples: vec![DomainKnowledgeRepresentativeExample {
                subject_id: "callable:create_incident".to_string(),
                evidence_fingerprint: "evidence-shape:incident-callable".to_string(),
                fact_kind: Some(NodeKind::Callable),
                label: Some("incident_triage.create_incident".to_string()),
            }],
        }
    }

    #[test]
    fn persisted_domain_knowledge_record_roundtrips_json() {
        let record = sample_domain_knowledge_record("domain-record:incident");

        let json = serde_json::to_string(&record).expect("serialize domain record");
        let roundtripped: DomainKnowledgeRecord =
            serde_json::from_str(&json).expect("deserialize domain record");

        assert_eq!(roundtripped, record);
    }

    #[test]
    fn persisted_domain_knowledge_selector_language_roundtrips_json() {
        let mut record = sample_domain_knowledge_record("domain-record:selector-language");
        record.selector.predicates = vec![
            DomainKnowledgeSelectorPredicate::FactKind(NodeKind::Callable),
            DomainKnowledgeSelectorPredicate::EdgeFactKind(EdgeKind::Calls),
            DomainKnowledgeSelectorPredicate::FieldEquals {
                path: "fact.qualified_name".to_string(),
                value: "incident_triage.create_incident".to_string(),
            },
            DomainKnowledgeSelectorPredicate::FieldContains {
                path: "fact.attributes".to_string(),
                value: "post".to_string(),
            },
            DomainKnowledgeSelectorPredicate::HasField {
                path: "fact.signature.parameters".to_string(),
            },
            DomainKnowledgeSelectorPredicate::PropertyExists {
                owner_path: "fact.normalized".to_string(),
                property_name: "member".to_string(),
            },
            DomainKnowledgeSelectorPredicate::SourceTargetRelationship {
                edge_kind: EdgeKind::Calls,
                source_kind: Some(NodeKind::Callable),
                target_kind: Some(NodeKind::ExternalTarget),
            },
            DomainKnowledgeSelectorPredicate::CallPath {
                path: DomainKnowledgeCallPathSelector {
                    start_kind: Some(NodeKind::Callable),
                    steps: vec![DomainKnowledgeCallPathStep {
                        call_kind: Some(CallEdgeKind::Direct),
                        target: DomainKnowledgeCallTargetPattern {
                            node_kind: Some(NodeKind::Callable),
                            qualified_name: Some("incident_triage.score_risk".to_string()),
                            name_contains: None,
                            external_package: None,
                        },
                    }],
                },
            },
            DomainKnowledgeSelectorPredicate::OrderedCalls {
                pattern: DomainKnowledgeOrderedCallPattern {
                    calls: vec![
                        DomainKnowledgeCallPattern {
                            target: DomainKnowledgeCallTargetPattern {
                                node_kind: Some(NodeKind::Callable),
                                qualified_name: Some("incident_triage.score_risk".to_string()),
                                name_contains: None,
                                external_package: None,
                            },
                            argument_contains: vec!["incident".to_string()],
                        },
                        DomainKnowledgeCallPattern {
                            target: DomainKnowledgeCallTargetPattern {
                                node_kind: Some(NodeKind::Callable),
                                qualified_name: Some(
                                    "incident_triage.persist_incident".to_string(),
                                ),
                                name_contains: None,
                                external_package: None,
                            },
                            argument_contains: Vec::new(),
                        },
                    ],
                    allow_intervening_calls: true,
                },
            },
            DomainKnowledgeSelectorPredicate::EvidenceFingerprint {
                fingerprint: "evidence-shape:incident-callable".to_string(),
            },
        ];

        let json = serde_json::to_string(&record).expect("serialize selector language");
        let roundtripped: DomainKnowledgeRecord =
            serde_json::from_str(&json).expect("deserialize selector language");

        assert_eq!(roundtripped.selector.predicates, record.selector.predicates);
    }

    #[test]
    fn persisted_domain_knowledge_record_statuses_roundtrip_json() {
        let statuses = [
            DomainKnowledgeStatus::Active,
            DomainKnowledgeStatus::Stale,
            DomainKnowledgeStatus::Superseded,
            DomainKnowledgeStatus::Unknown,
            DomainKnowledgeStatus::Missing,
        ];

        for status in statuses {
            let mut record = sample_domain_knowledge_record(&format!("domain-record:{status:?}"));
            record.status = status;

            let json = serde_json::to_string(&record).expect("serialize domain record status");
            let roundtripped: DomainKnowledgeRecord =
                serde_json::from_str(&json).expect("deserialize domain record status");

            assert_eq!(roundtripped.status, status);
        }
    }

    #[test]
    fn persisted_domain_knowledge_records_are_orderable_for_stable_comparison() {
        let earlier = sample_domain_knowledge_record("domain-record:a");
        let later = sample_domain_knowledge_record("domain-record:b");

        let mut records = vec![later.clone(), earlier.clone()];
        records.sort();

        assert_eq!(records, vec![earlier, later]);
    }

    #[test]
    fn persisted_domain_knowledge_record_deserializes_with_migration_defaults() {
        let value = serde_json::json!({
            "record_id": "domain-record:migrated",
            "selector": {
                "selector_id": "selector:migrated",
                "description": "legacy record selector"
            },
            "label": {
                "name": "Legacy label"
            }
        });

        let record: DomainKnowledgeRecord =
            serde_json::from_value(value).expect("deserialize legacy domain record");
        let migrated = migrate_domain_knowledge_record(record);

        assert_eq!(
            migrated.record_schema_version,
            DOMAIN_KNOWLEDGE_RECORD_SCHEMA_VERSION
        );
        assert_eq!(migrated.status, DomainKnowledgeStatus::Unknown);
        assert_eq!(migrated.selector.predicates, Vec::new());
        assert_eq!(
            migrated.selector.expected_match,
            DomainKnowledgeMatchExpectation::Any
        );
        assert!(migrated.selector.expected_fact_kinds.is_empty());
        assert_eq!(
            migrated.compatible_supergraph_schema_versions,
            vec![SCHEMA_VERSION.to_string()]
        );
        assert!(migrated.last_seen_evidence_fingerprints.is_empty());
        assert!(migrated.accepted_representative_examples.is_empty());
        assert_eq!(migrated.provenance.source, DomainKnowledgeSource::User);
    }
}
