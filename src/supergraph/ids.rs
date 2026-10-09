//! Compact, typed, `Copy` identifiers.
//!
//! Every id derived by `stable_id` is a 64-bit FNV-1a hash plus a kind prefix. The textual form
//! (`prefix:hex16`) is only produced by `Display`/serde; in memory an id is 8 bytes
//! (typed ids) or 16 bytes ([`NodeId`], which also carries a [`Tag`]).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};
use std::num::NonZeroU64;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Error returned when parsing an id from its textual form fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIdError(String);

impl fmt::Display for ParseIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid id: {}", self.0)
    }
}

impl std::error::Error for ParseIdError {}

/// Incremental FNV-1a hasher matching the historical `stable_id` algorithm.
///
/// The textual prefix is *not* hashed; each part is terminated by a `0xff` separator.
#[derive(Debug, Clone, Copy)]
pub struct IdHasher {
    hash: u64,
}

impl Default for IdHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl IdHasher {
    pub const fn new() -> Self {
        Self {
            hash: FNV_OFFSET_BASIS,
        }
    }

    pub fn reset(&mut self) {
        self.hash = FNV_OFFSET_BASIS;
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }

    /// Terminates the current part.
    pub fn finish_part(&mut self) {
        self.hash ^= 0xff;
        self.hash = self.hash.wrapping_mul(FNV_PRIME);
    }

    pub fn part_str(&mut self, part: &str) {
        self.write_bytes(part.as_bytes());
        self.finish_part();
    }

    /// Hashes another id as its textual form, so the result equals hashing `id.to_string()`.
    pub fn part_id(&mut self, id: NodeId) {
        self.write_bytes(id.tag.prefix().as_bytes());
        self.write_bytes(b":");
        self.write_bytes(&hex16(id.hash.get()));
        self.finish_part();
    }

    pub fn part_prefixed(&mut self, prefix: &str, hash: u64) {
        self.write_bytes(prefix.as_bytes());
        self.write_bytes(b":");
        self.write_bytes(&hex16(hash));
        self.finish_part();
    }

    /// Returns the (never zero) hash. Every part must already be terminated.
    pub fn finish(self) -> NonZeroU64 {
        NonZeroU64::new(self.hash).unwrap_or(NonZeroU64::MIN)
    }
}

/// One input to [`hash_parts`].
#[derive(Debug, Clone, Copy)]
pub enum IdPart<'a> {
    Str(&'a str),
    Id(NodeId),
    /// An id of a non-node kind (`edge`, `fact`, `payload`), hashed as its `prefix:hex16` text.
    Prefixed(&'static str, u64),
    /// Hashed as its decimal text, matching `n.to_string()`.
    U64(u64),
}

impl<'a> From<&'a str> for IdPart<'a> {
    fn from(value: &'a str) -> Self {
        IdPart::Str(value)
    }
}

impl<'a> From<&'a String> for IdPart<'a> {
    fn from(value: &'a String) -> Self {
        IdPart::Str(value)
    }
}

impl<'a> From<&'a NodeId> for IdPart<'a> {
    fn from(value: &'a NodeId) -> Self {
        IdPart::Id(*value)
    }
}

impl<'a> From<&&'a str> for IdPart<'a> {
    fn from(value: &&'a str) -> Self {
        IdPart::Str(value)
    }
}

impl From<crate::intern::Sym> for IdPart<'_> {
    fn from(value: crate::intern::Sym) -> Self {
        IdPart::Str(value.as_str())
    }
}

impl From<&crate::intern::Sym> for IdPart<'_> {
    fn from(value: &crate::intern::Sym) -> Self {
        IdPart::Str(value.as_str())
    }
}

impl From<NodeId> for IdPart<'_> {
    fn from(value: NodeId) -> Self {
        IdPart::Id(value)
    }
}

impl From<u64> for IdPart<'_> {
    fn from(value: u64) -> Self {
        IdPart::U64(value)
    }
}

/// Builds a `&[IdPart]` from heterogeneous parts (`&str`, `&String`, ids, `u64`).
#[macro_export]
macro_rules! id_parts {
    ($($part:expr),* $(,)?) => {
        &[$($crate::supergraph::ids::IdPart::from($part)),*]
    };
}

/// Id of a callable from its qualified name (or `"{module_path}:<module>"` for a module
/// initializer).
pub fn callable_id_from_text(text: &str) -> NodeId {
    NodeId::stable(Tag::Callable, &[IdPart::Str(text)])
}

/// Id of an external name (`anyhow.Context`), as used by import bindings.
pub fn external_name_id(text: &str) -> NodeId {
    NodeId::stable(Tag::ExternalTarget, &[IdPart::Str(text)])
}

pub fn hash_parts(parts: &[IdPart<'_>]) -> NonZeroU64 {
    let mut hasher = IdHasher::new();
    for part in parts {
        match *part {
            IdPart::Str(text) => hasher.part_str(text),
            IdPart::Id(id) => hasher.part_id(id),
            IdPart::Prefixed(prefix, hash) => hasher.part_prefixed(prefix, hash),
            IdPart::U64(value) => hasher.part_str(&value.to_string()),
        }
    }
    hasher.finish()
}

fn hex16(value: u64) -> [u8; 16] {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 16];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = DIGITS[((value >> (60 - 4 * index)) & 0xf) as usize];
    }
    out
}

fn parse_hash(text: &str) -> Result<NonZeroU64, ParseIdError> {
    if text.len() != 16 {
        return Err(ParseIdError(text.to_string()));
    }
    u64::from_str_radix(text, 16)
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or_else(|| ParseIdError(text.to_string()))
}

macro_rules! tags {
    ($($variant:ident => $prefix:literal),+ $(,)?) => {
        /// Kind of a node id; distinguishes ids in heterogeneous positions.
        ///
        /// Variants are declared in the order of their `prefix:` text so that `Ord` on [`NodeId`]
        /// matches the historical string ordering of ids.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u8)]
        pub enum Tag {
            $($variant),+
        }

        impl Tag {
            pub const ALL: &'static [Tag] = &[$(Tag::$variant),+];

            pub const fn prefix(self) -> &'static str {
                match self {
                    $(Tag::$variant => $prefix),+
                }
            }

            pub fn from_prefix(prefix: &str) -> Option<Tag> {
                match prefix {
                    $($prefix => Some(Tag::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

tags! {
    Artifact => "artifact",
    BasicBlock => "basic-block",
    Binding => "binding",
    CallSite => "call-site",
    Callable => "callable",
    CfgNode => "cfg-node",
    Condition => "condition",
    Definition => "definition",
    DataFlowNode => "df-node",
    Diagnostic => "diagnostic",
    DomainKnowledge => "domain-knowledge",
    Edge => "edge",
    Expression => "expression",
    ExternalTarget => "external-target",
    External => "external",
    Requirement => "requirement",
    Scope => "scope",
    Statement => "statement",
    Symbol => "symbol",
    Use => "use",
    Value => "value",
}

/// Id for a position that can hold any kind of node. 16 bytes; `Option<NodeId>` is also 16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId {
    pub tag: Tag,
    pub hash: NonZeroU64,
}

impl std::hash::Hash for NodeId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash.get());
    }
}

impl NodeId {
    pub const fn new(tag: Tag, hash: NonZeroU64) -> Self {
        Self { tag, hash }
    }

    /// Builds an id the same way the historical `stable_id(tag.prefix(), parts)` did.
    pub fn stable(tag: Tag, parts: &[IdPart<'_>]) -> Self {
        Self::new(tag, hash_parts(parts))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{:016x}", self.tag.prefix(), self.hash.get())
    }
}

impl FromStr for NodeId {
    type Err = ParseIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (prefix, hex) = text
            .split_once(':')
            .ok_or_else(|| ParseIdError(text.to_string()))?;
        let tag = Tag::from_prefix(prefix).ok_or_else(|| ParseIdError(text.to_string()))?;
        Ok(NodeId::new(tag, parse_hash(hex)?))
    }
}

impl Serialize for NodeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<str>::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// Defines an 8-byte typed id with a fixed textual prefix.
macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub NonZeroU64);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn stable(parts: &[IdPart<'_>]) -> Self {
                Self(hash_parts(parts))
            }

            pub const fn hash(self) -> NonZeroU64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}:{:016x}", $prefix, self.0.get())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                text.strip_prefix(concat!($prefix, ":"))
                    .ok_or_else(|| ParseIdError(text.to_string()))
                    .and_then(parse_hash)
                    .map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = std::borrow::Cow::<str>::deserialize(deserializer)?;
                text.parse().map_err(de::Error::custom)
            }
        }
    };
}

/// A typed id whose kind is a node [`Tag`]; convertible to and from [`NodeId`].
macro_rules! node_typed_id {
    ($(#[$meta:meta])* $name:ident, $tag:ident, $prefix:literal) => {
        typed_id!($(#[$meta])* $name, $prefix);

        impl From<$name> for IdPart<'_> {
            fn from(id: $name) -> Self {
                IdPart::Id(NodeId::from(id))
            }
        }

        impl<'a> From<&'a $name> for IdPart<'a> {
            fn from(id: &'a $name) -> Self {
                IdPart::Id(NodeId::from(*id))
            }
        }

        impl From<$name> for NodeId {
            fn from(id: $name) -> NodeId {
                NodeId::new(Tag::$tag, id.0)
            }
        }

        impl TryFrom<NodeId> for $name {
            type Error = WrongTag;

            fn try_from(id: NodeId) -> Result<Self, WrongTag> {
                if id.tag == Tag::$tag {
                    Ok(Self(id.hash))
                } else {
                    Err(WrongTag { expected: Tag::$tag, found: id.tag })
                }
            }
        }

        impl PartialEq<NodeId> for $name {
            fn eq(&self, other: &NodeId) -> bool {
                other.tag == Tag::$tag && other.hash == self.0
            }
        }

        impl PartialEq<$name> for NodeId {
            fn eq(&self, other: &$name) -> bool {
                other == self
            }
        }
    };
}

/// Returned when converting a [`NodeId`] into a typed id of a different kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrongTag {
    pub expected: Tag,
    pub found: Tag,
}

impl fmt::Display for WrongTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected a {} id, found a {} id", self.expected.prefix(), self.found.prefix())
    }
}

impl std::error::Error for WrongTag {}

node_typed_id!(ArtifactId, Artifact, "artifact");
node_typed_id!(ScopeId, Scope, "scope");
node_typed_id!(BindingId, Binding, "binding");
node_typed_id!(CallableId, Callable, "callable");
node_typed_id!(CallSiteId, CallSite, "call-site");
node_typed_id!(ExternalTargetId, ExternalTarget, "external-target");
node_typed_id!(StatementId, Statement, "statement");
node_typed_id!(ExpressionId, Expression, "expression");
node_typed_id!(ConditionId, Condition, "condition");
node_typed_id!(SymbolId, Symbol, "symbol");
node_typed_id!(DefinitionId, Definition, "definition");
node_typed_id!(UseId, Use, "use");
node_typed_id!(ValueId, Value, "value");
node_typed_id!(BasicBlockId, BasicBlock, "basic-block");
node_typed_id!(DomainKnowledgeId, DomainKnowledge, "domain-knowledge");
node_typed_id!(CfgNodeId, CfgNode, "cfg-node");
node_typed_id!(DataFlowNodeId, DataFlowNode, "df-node");
node_typed_id!(RequirementId, Requirement, "requirement");
node_typed_id!(DiagnosticId, Diagnostic, "diagnostic");

typed_id!(
    /// Id of a graph edge.
    EdgeId,
    "edge"
);
impl From<EdgeId> for NodeId {
    fn from(id: EdgeId) -> NodeId {
        NodeId::new(Tag::Edge, id.0)
    }
}

impl TryFrom<NodeId> for EdgeId {
    type Error = WrongTag;

    fn try_from(id: NodeId) -> Result<Self, WrongTag> {
        if id.tag == Tag::Edge {
            Ok(Self(id.hash))
        } else {
            Err(WrongTag { expected: Tag::Edge, found: id.tag })
        }
    }
}

typed_id!(
    /// Identity hash of a node or edge, including its payload and provenance.
    FactId,
    "fact"
);
typed_id!(
    /// Hash of a node or edge payload.
    PayloadHash,
    "payload"
);

macro_rules! prefixed_part {
    ($($name:ident),+) => {$(
        impl From<$name> for IdPart<'_> {
            fn from(id: $name) -> Self {
                IdPart::Prefixed($name::PREFIX, id.0.get())
            }
        }

        impl From<&$name> for IdPart<'_> {
            fn from(id: &$name) -> Self {
                IdPart::Prefixed($name::PREFIX, id.0.get())
            }
        }
    )+};
}

prefixed_part!(EdgeId, FactId, PayloadHash);

/// Id helpers for unit tests, which used to use readable strings such as `"caller"` as ids.
#[cfg(test)]
pub mod test_support {
    use super::*;

    /// A deterministic node id derived from a readable name (tag: artifact).
    pub fn test_id(name: &str) -> NodeId {
        NodeId::stable(Tag::Artifact, &[IdPart::Str(name)])
    }

    /// A deterministic node id of the given kind derived from a readable name.
    pub fn test_id_of(tag: Tag, name: &str) -> NodeId {
        NodeId::stable(tag, &[IdPart::Str(name)])
    }

    /// A deterministic edge id derived from a readable name.
    pub fn test_edge_id(name: &str) -> EdgeId {
        EdgeId::stable(&[IdPart::Str(name)])
    }
}

/// Ids are already uniformly distributed hashes; hashing them again is wasted work.
#[derive(Default, Clone, Copy)]
pub struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.0 = self.0.rotate_left(8) ^ u64::from(value);
    }

    fn write_u64(&mut self, value: u64) {
        self.0 ^= value;
    }
}

pub type IdBuildHasher = BuildHasherDefault<IdentityHasher>;
pub type IdMap<K, V> = HashMap<K, V, IdBuildHasher>;
pub type IdSet<K> = HashSet<K, IdBuildHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn sizes_are_compact() {
        assert_eq!(size_of::<ExpressionId>(), 8);
        assert_eq!(size_of::<Option<ExpressionId>>(), 8);
        assert_eq!(size_of::<EdgeId>(), 8);
        assert_eq!(size_of::<NodeId>(), 16);
    }

    #[test]
    fn hashing_an_id_equals_hashing_its_text() {
        let artifact = NodeId::stable(Tag::Artifact, &[IdPart::Str("x.rs")]);
        let text = artifact.to_string();
        let by_id = NodeId::stable(Tag::Scope, &[IdPart::Id(artifact), IdPart::Str("m")]);
        let by_text = NodeId::stable(Tag::Scope, &[IdPart::Str(&text), IdPart::Str("m")]);
        assert_eq!(by_id, by_text);
        let by_number = NodeId::stable(Tag::Scope, &[IdPart::U64(42)]);
        let by_number_text = NodeId::stable(Tag::Scope, &[IdPart::Str("42")]);
        assert_eq!(by_number, by_number_text);
    }

    #[test]
    fn text_round_trips() {
        let id = NodeId::stable(Tag::BasicBlock, &[IdPart::Str("q")]);
        assert_eq!(id.to_string().parse::<NodeId>().unwrap(), id);
        let edge = EdgeId::stable(&[IdPart::Str("q")]);
        assert_eq!(edge.to_string().parse::<EdgeId>().unwrap(), edge);
        assert!("edge:123".parse::<EdgeId>().is_err());
        assert!("bogus:0000000000000001".parse::<NodeId>().is_err());
        assert!("scope:0000000000000000".parse::<NodeId>().is_err());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<NodeId>(&json).unwrap(), id);
    }

    #[test]
    fn tag_conversions_check_the_kind() {
        let expression = ExpressionId::stable(&[IdPart::Str("e")]);
        let node = NodeId::from(expression);
        assert_eq!(node.tag, Tag::Expression);
        assert_eq!(ExpressionId::try_from(node), Ok(expression));
        assert!(ValueId::try_from(node).is_err());
        assert_eq!(node.to_string(), expression.to_string());
        assert!(expression == node);
    }

    #[test]
    fn every_tag_prefix_round_trips() {
        for tag in Tag::ALL {
            assert_eq!(Tag::from_prefix(tag.prefix()), Some(*tag));
        }
    }

    #[test]
    fn identity_maps_work() {
        let mut map: IdMap<NodeId, u32> = IdMap::default();
        let a = NodeId::stable(Tag::Value, &[IdPart::Str("a")]);
        let b = NodeId::stable(Tag::Value, &[IdPart::Str("b")]);
        map.insert(a, 1);
        map.insert(b, 2);
        assert_eq!(map[&a], 1);
        assert_eq!(map[&b], 2);
    }
}
