use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::ast::{CallContext, SourceSpan};
use crate::intern::Sym;

pub const SCHEMA_VERSION: &str = "program-supergraph.v3";
pub const DOMAIN_KNOWLEDGE_RECORD_SCHEMA_VERSION: &str = "domain-knowledge-record.v1";

pub use super::ids::{EdgeId, FactId, NodeId, PayloadHash};
use super::ids::{IdMap, IdSet};
use super::multimap::MultiMap;

/// Free-form identifier used by persisted domain-knowledge records (not a graph id).
pub type StableId = String;
pub type SubjectId = StableId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ProgramSupergraphWire")]
pub struct ProgramSupergraph {
    pub schema_version: String,
    pub language: String,
    pub root: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Derived lookup tables; never serialized, rebuilt on load.
    #[serde(skip)]
    pub indexes: GraphIndexes,
    #[serde(skip)]
    pub(crate) id_cache: IdCache,
}

/// Wire form read by `ProgramSupergraph::deserialize`.
///
/// `strings` (v3 compact form) must precede `nodes` and `edges`: it installs the table that
/// numeric `Sym` values resolve against while the later fields are parsed.
#[derive(Deserialize)]
struct ProgramSupergraphWire {
    schema_version: String,
    language: String,
    root: String,
    #[allow(dead_code)]
    strings: Option<StringTableWire>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

struct StringTableWire;

impl<'de> Deserialize<'de> for StringTableWire {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let strings = Vec::<String>::deserialize(deserializer)?;
        crate::intern::install_read_table(&strings);
        Ok(StringTableWire)
    }
}

impl TryFrom<ProgramSupergraphWire> for ProgramSupergraph {
    type Error = String;

    fn try_from(wire: ProgramSupergraphWire) -> Result<Self, String> {
        crate::intern::clear_read_table();
        if wire.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version `{}` (expected {SCHEMA_VERSION})",
                wire.schema_version
            ));
        }
        let indexes = crate::supergraph::builder::build_indexes(&wire.nodes, &wire.edges);
        Ok(Self {
            schema_version: wire.schema_version,
            language: wire.language,
            root: wire.root,
            nodes: wire.nodes,
            edges: wire.edges,
            indexes,
            id_cache: IdCache::default(),
        })
    }
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
    node_ids: IdSet<NodeId>,
    edge_ids: IdSet<EdgeId>,
}

impl PartialEq for IdCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for IdCache {}

impl ProgramSupergraph {
    /// Writes the compact v3 form: a sorted `strings` table followed by nodes and edges in
    /// which every interned string is a table position. Indexes are not written.
    pub fn write_json<W: std::io::Write>(&self, out: &mut W) -> std::io::Result<()> {
        self.write_json_impl(out, true)
    }

    /// Writes the expanded form (strings inline, no table), byte-identical to
    /// `serde_json::to_writer(self)`.
    pub fn write_json_expanded<W: std::io::Write>(&self, out: &mut W) -> std::io::Result<()> {
        self.write_json_impl(out, false)
    }

    fn write_json_impl<W: std::io::Write>(&self, out: &mut W, table_form: bool) -> std::io::Result<()> {
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

        let table = table_form.then(crate::intern::StringTable::snapshot);
        let table = table.as_ref();
        let chunk_json = move |chunk_is_node: bool, node_chunk: &[GraphNode], edge_chunk: &[GraphEdge]| {
            let run = || {
                if chunk_is_node {
                    serialize_chunk(node_chunk)
                } else {
                    serialize_chunk(edge_chunk)
                }
            };
            match table {
                Some(table) => table.serialize_with(run),
                None => run(),
            }
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let node_chunk = self.nodes.len().div_ceil(threads).max(1024);
        let edge_chunk = self.edges.len().div_ceil(threads).max(1024);
        std::thread::scope(|scope| {
            let nodes = self
                .nodes
                .chunks(node_chunk)
                .map(|chunk| scope.spawn(move || chunk_json(true, chunk, &[])))
                .collect::<Vec<_>>();
            let edges = self
                .edges
                .chunks(edge_chunk)
                .map(|chunk| scope.spawn(move || chunk_json(false, &[], chunk)))
                .collect::<Vec<_>>();

            out.write_all(b"{\"schema_version\":")?;
            serde_json::to_writer(&mut *out, &self.schema_version)?;
            out.write_all(b",\"language\":")?;
            serde_json::to_writer(&mut *out, &self.language)?;
            out.write_all(b",\"root\":")?;
            serde_json::to_writer(&mut *out, &self.root)?;
            if let Some(table) = table {
                out.write_all(b",\"strings\":")?;
                serde_json::to_writer(&mut *out, &table.strings)?;
            }
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
            out.write_all(b"}")
        })
    }

    /// Drops the lazily built id membership sets (memory probe / after bulk mutation).
    #[doc(hidden)]
    pub fn drop_id_cache(&mut self) {
        self.id_cache = IdCache::default();
    }

    pub(crate) fn invalidate_id_cache(&mut self) {
        self.id_cache = IdCache::default();
    }

    /// Pushes `node` unless a node with the same id exists. Returns whether it was added.
    pub(crate) fn push_node_if_new(&mut self, node: GraphNode) -> bool {
        let cache = &mut self.id_cache;
        if cache.node_len != self.nodes.len() || cache.node_ids.len() != self.nodes.len() {
            cache.node_ids = self.nodes.iter().map(|node| node.node_id).collect();
        }
        if !cache.node_ids.insert(node.node_id) {
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
            cache.edge_ids = self.edges.iter().map(|edge| edge.edge_id).collect();
        }
        if !cache.edge_ids.insert(edge.edge_id) {
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
    pub fact_id: Option<FactId>,
    pub payload_hash: Option<PayloadHash>,
    pub kind: NodeKind,
    pub owner: SourceOwnership,
    pub span: Option<SourceSpan>,
    pub confidence: Confidence,
    pub uncertainty: Uncertainty,
    pub evidence: Vec<Evidence>,
    pub fact: NodeFact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub edge_id: EdgeId,
    pub fact_id: Option<FactId>,
    pub payload_hash: Option<PayloadHash>,
    pub kind: EdgeKind,
    pub source_id: NodeId,
    pub target_id: Option<NodeId>,
    pub owner: SourceOwnership,
    pub span: Option<SourceSpan>,
    pub confidence: Confidence,
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
    Scope(Box<Scope>),
    Binding(Binding),
    Callable(Box<Callable>),
    CallSite(Box<CallSite>),
    ExternalTarget(ExternalTarget),
    Statement(Box<Statement>),
    Expression(Box<Expression>),
    Condition(Box<Condition>),
    Symbol(Symbol),
    Definition(Definition),
    Use(Use),
    Value(Value),
    BasicBlock(Box<BasicBlock>),
    DomainKnowledge(Box<DomainKnowledge>),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub summary: Sym,
    pub source_id: Option<NodeId>,
    pub source_span: Option<SourceSpan>,
    pub content_hash: Option<Sym>,
    pub syntax: Option<SyntaxReference>,
}

impl Serialize for Evidence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Evidence", 6)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("summary", &self.summary)?;
        state.serialize_field("source_id", &self.source_id)?;
        state.serialize_field("source_span", &self.source_span)?;
        state.serialize_field("content_hash", &self.content_hash)?;
        let syntax = self.syntax.as_ref().map(|syntax| SyntaxReferenceView {
            kind: syntax.kind,
            node_key: syntax.node_key(self.source_span),
            field_path: &*syntax.field_path,
        });
        state.serialize_field("syntax", &syntax)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Evidence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            kind: EvidenceKind,
            summary: Sym,
            source_id: Option<NodeId>,
            source_span: Option<SourceSpan>,
            content_hash: Option<Sym>,
            syntax: Option<RawSyntax>,
        }
        #[derive(Deserialize)]
        struct RawSyntax {
            kind: Sym,
            node_key: Option<String>,
                    field_path: Vec<Sym>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Evidence {
            kind: raw.kind,
            summary: raw.summary,
            source_id: raw.source_id,
            source_span: raw.source_span,
            content_hash: raw.content_hash,
            syntax: raw.syntax.map(|syntax| {
                SyntaxReference {
                    kind: syntax.kind,
                    // The node key is `{prefix}:{span key}`; only the prefix is stored.
                    key_prefix: syntax
                        .node_key
                        .as_deref()
                        .and_then(|key| key.split_once(':'))
                        .map(|(prefix, _)| Sym::new(prefix)),
                    field_path: syntax.field_path.into(),
                }
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EvidenceKind {
    Parser,
    Resolver,
    Adapter,
    Inference,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SyntaxReference {
    pub kind: Sym,
    /// Prefix of the serialized `node_key` (`{prefix}:{span key}`); the key itself is derived
    /// from the owning evidence's source span instead of being stored per item.
    pub key_prefix: Option<Sym>,
    pub field_path: Box<[Sym]>,
}

impl SyntaxReference {
    pub fn node_key(&self, span: Option<SourceSpan>) -> Option<String> {
        let (prefix, span) = (self.key_prefix?, span?);
        Some(format!(
            "{prefix}:{}:{}:{}:{}:{}:{}",
            span.start_byte, span.end_byte, span.start_row, span.start_column, span.end_row, span.end_column
        ))
    }
}

#[derive(Serialize)]
struct SyntaxReferenceView<'a> {
    kind: Sym,
    node_key: Option<String>,
    field_path: &'a [Sym],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: NodeId,
    pub path: Sym,
    pub module_path: Sym,
    pub content_hash: Option<Sym>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub scope_id: NodeId,
    pub parent_scope_id: Option<NodeId>,
    pub artifact_id: NodeId,
    pub kind: ScopeKind,
    pub variant: ScopeVariant,
    pub language_variant: Option<Sym>,
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
    pub name: Sym,
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
    /// Qualified class name.
    Class(Sym),
    /// Module path.
    Module(Sym),
    /// Qualified external name.
    External(Sym),
    Value(Sym),
    Unresolved(Sym),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callable {
    pub callable_id: NodeId,
    pub kind: CallableKind,
    pub name: Option<Sym>,
    pub qualified_name: Sym,
    pub artifact_id: NodeId,
    pub declaration_span: SourceSpan,
    pub body_span: Option<SourceSpan>,
    pub signature: Signature,
    pub scope_id: NodeId,
    pub attributes: Vec<Sym>,
    pub incoming_local_call_count: usize,
    pub external_invocation_metadata: Vec<Sym>,
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
    pub parameters: Vec<Sym>,
    pub return_annotation: Option<Sym>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSite {
    pub call_site_id: NodeId,
    pub artifact_id: NodeId,
    pub enclosing_callable_id: NodeId,
    pub span: SourceSpan,
    pub callee_expression: Sym,
    pub argument_shape: ArgumentShape,
    pub dispatch_kind: DispatchKind,
    pub context: CallContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgumentShape {
    pub positional_count: usize,
    pub named_arguments: Vec<Sym>,
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
    pub ecosystem: Sym,
    pub package_name: Option<Sym>,
    pub package_version: Option<Sym>,
    pub module_path: Option<Sym>,
    pub qualified_name: Sym,
    pub member_path: Option<Sym>,
    pub target_kind: ExternalTargetKind,
    pub source: Sym,
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
    pub description: Sym,
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
    pub original_text: Option<Sym>,
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
    pub canonical: Option<Sym>,
    pub operator: Option<Sym>,
    pub identifier: Option<Sym>,
    pub member: Option<Sym>,
    pub literal: Option<ValueLiteral>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub condition_id: NodeId,
    pub callable_id: NodeId,
    pub statement_id: Option<NodeId>,
    pub expression_id: Option<NodeId>,
    pub kind: ConditionKind,
    pub controlled_statement_ids: Vec<NodeId>,
    pub outcome_labels: Vec<Sym>,
    pub regions: Vec<ControlRegion>,
    pub continuation: Option<ContinuationPoint>,
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
    pub label: Sym,
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
    pub description: Sym,
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
    pub name: Sym,
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
    pub name: Option<Sym>,
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
    pub name: Option<Sym>,
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
    pub role: ValueRole,
    pub symbol_id: Option<NodeId>,
    pub expression_id: Option<NodeId>,
    pub call_site_id: Option<NodeId>,
    pub name: Option<Sym>,
    pub ordinal: Option<usize>,
    pub state_of_value_id: Option<NodeId>,
    pub type_hint: Option<Sym>,
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
    pub label: Sym,
    pub semantic_kind: Option<Sym>,
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
    pub name: Option<Sym>,
    pub text: Sym,
    pub semantic_kind: Option<Sym>,
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
    pub title: Sym,
    pub summary: Sym,
    pub source_rule: Sym,
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
    pub summary: Sym,
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
    pub unresolved_target: Option<Sym>,
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
    pub precision: Sym,
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
    pub label: Sym,
    pub ordinal: usize,
    pub region_kind: Option<ControlRegionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controls {
    pub callable_id: NodeId,
    pub condition_id: NodeId,
    pub controlled_id: NodeId,
    pub precision: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defines {
    pub callable_id: NodeId,
    pub definition_id: NodeId,
    pub name: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    pub callable_id: NodeId,
    pub use_id: NodeId,
    pub name: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFlow {
    pub callable_id: NodeId,
    pub name: Sym,
    pub flow_kind: DataFlowKind,
    pub precision: Sym,
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
    pub parameter_name: Sym,
    pub ordinal: usize,
    pub precision: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnsTo {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    pub precision: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterOut {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    pub parameter_name: Sym,
    pub ordinal: usize,
    pub precision: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThrowsTo {
    pub call_site_id: NodeId,
    pub caller_callable_id: NodeId,
    pub callee_callable_id: NodeId,
    #[serde(default)]
    pub target_kind: ThrowsToTargetKind,
    #[serde(default)]
    pub exception_value: Option<Sym>,
    #[serde(default)]
    pub exception_type: Option<Sym>,
    pub precision: Sym,
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
    pub order_key: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracesTo {
    pub requirement_id: NodeId,
    pub code_fact_id: NodeId,
    pub precision: Sym,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependsOnDomainKnowledge {
    pub requirement_id: NodeId,
    pub domain_knowledge_id: NodeId,
    pub precision: Sym,
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
            self.artifact_id.map(|id| id.to_string()).unwrap_or_default(),
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
            part.parse::<u32>()
                .map_err(|_| de::Error::custom("invalid source span index key number"))
        };

        Ok(Self {
            artifact_id: if artifact.is_empty() {
                None
            } else {
                Some(artifact.parse().map_err(de::Error::custom)?)
            },
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphIndexes {
    pub node_position_by_id: IdMap<NodeId, u32>,
    pub edge_position_by_id: IdMap<EdgeId, u32>,
    pub nodes_by_kind: MultiMap<NodeKind, NodeId>,
    pub edges_by_kind: MultiMap<EdgeKind, EdgeId>,
    pub nodes_by_uncertainty: MultiMap<Uncertainty, NodeId>,
    pub edges_by_uncertainty: MultiMap<Uncertainty, EdgeId>,
    pub outgoing_edges_by_node: MultiMap<NodeId, EdgeId>,
    pub incoming_edges_by_node: MultiMap<NodeId, EdgeId>,
    pub source_span_to_nodes: MultiMap<SourceSpanIndexKey, NodeId>,
    pub artifact_to_nodes: MultiMap<NodeId, NodeId>,
    pub callable_to_nodes: MultiMap<NodeId, NodeId>,
    pub owner_to_nodes: MultiMap<NodeId, NodeId>,
    pub owner_to_edges: MultiMap<NodeId, EdgeId>,
    pub symbol_to_definitions: MultiMap<NodeId, NodeId>,
    pub symbol_to_uses: MultiMap<NodeId, NodeId>,
    pub requirement_to_code: MultiMap<NodeId, NodeId>,
    pub code_to_requirements: MultiMap<NodeId, NodeId>,
    pub requirement_to_domain_knowledge: MultiMap<NodeId, NodeId>,
    pub domain_knowledge_to_requirements: MultiMap<NodeId, NodeId>,
    pub calls_by_caller: MultiMap<NodeId, EdgeId>,
    pub calls_by_concrete_target: MultiMap<NodeId, EdgeId>,
    pub call_site_to_calls: MultiMap<NodeId, EdgeId>,
    pub caller_to_concrete_target_calls: BTreeMap<NodeId, BTreeMap<NodeId, Vec<EdgeId>>>,
    pub caller_to_concrete_call_targets: MultiMap<NodeId, NodeId>,
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
