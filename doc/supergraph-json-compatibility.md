# Program Supergraph JSON Compatibility

This note defines the compatibility rules for persisted `ProgramSupergraph` JSON
emitted by the public library APIs and the `req-graph supergraph-python` /
`req-graph supergraph-typescript` CLI commands.

The JSON shape is a durable interchange format for materialized graph snapshots.
It is not the only possible fact store. Long-lived stores may persist normalized
facts and materialize this snapshot shape for consumers, but they should follow
the same identity, provenance, uncertainty, and migration rules.

## Versioning Contract

`ProgramSupergraph.schema_version` identifies the serialized schema family. The
current version is `program-supergraph.v2`.

`ProgramSupergraph.language` identifies the single source language represented by
the snapshot. A persisted supergraph is not a mixed-language container; producers
must build separate snapshots for separate languages.

Within a schema family, consumers should accept additive changes:

- new optional fields with serde defaults;
- new payload fields whose absence can be backfilled from existing facts;
- new node, edge, diagnostic, condition, value, or outcome enum variants when
  consumers do not require exhaustive interpretation;
- new indexes derived from `nodes` and `edges`;
- additional provenance, evidence, confidence, uncertainty, or diagnostic
  records.

Breaking changes require a new `schema_version` and migration notes. Examples
include renaming or removing required fields, changing ID meaning, changing the
serialized representation of an existing field, changing `SourceSpanIndexKey`
encoding, changing a node or edge family so existing variants mean something
different, or making indexes canonical when they were previously derived.

Consumers should check `schema_version` before loading a snapshot. Unknown major
families must fail closed instead of being interpreted as a known schema.

## Stable Subjects And Immutable Facts

Graph node and edge IDs are stable subject IDs:

- `GraphNode.node_id` identifies the conceptual node.
- `GraphEdge.edge_id` identifies the conceptual edge.
- references inside facts and indexes use those stable subject IDs.

`fact_id` and `payload_hash` identify the exact serialized fact payload. They are
immutable version markers, not traversal references. If a callable body,
evidence, provenance, confidence, uncertainty, or endpoint changes, the subject
ID can remain stable while `fact_id` and `payload_hash` change.

Older snapshots may omit `fact_id` and `payload_hash`.
They default to empty strings on deserialization and should be recomputed by the
normal graph refresh/materialization pipeline before persistence or comparison.
Migration code should never invent subject churn solely because these fields were
missing.

`program-supergraph.v2` is a breaking cleanup from `program-supergraph.v1`. It
keeps a single top-level `language` and removes duplicated parser, analysis, and
nested language metadata from artifact, evidence, syntax, and scope payloads.

## Defaulted And Backfilled Fields

Fields annotated in the Rust schema with `#[serde(default)]` are intentionally
backward-compatible. When an older snapshot omits one of these fields, consumers
should treat the default as an unknown or conservative value until the graph is
refreshed.

Current examples include:

- `GraphNode.fact_id`, `GraphNode.payload_hash`, `GraphNode.uncertainty`;
- `GraphEdge.fact_id`, `GraphEdge.payload_hash`, `GraphEdge.uncertainty`;
- scope normalization metadata such as `Scope.variant`, `Scope.language_variant`,
  and `Scope.binding_behavior`;
- statement/expression/value additions such as control effects, value role,
  call-site references, names, ordinals, and mutable state references;
- structured control additions such as condition regions, continuation,
  fallthrough, CFG outcomes, branch arms, and path-condition references;
- `ThrowsTo` target metadata and exception text/type fields;
- requirement path-condition summaries.

Default values usually mean "not recorded by this producer", not "the producer
proved absence". Consumers should distinguish missing/defaulted facts from exact
negative evidence.

## Node And Edge Families

Node and edge `kind` values are compatibility gates. Consumers that implement a
logical view should filter by the families they understand and keep unknown
families available for roundtrip or diagnostics when possible.

Adding a new kind is additive when existing kinds keep their meaning and existing
views can ignore the new family without corrupting answers. Reusing a kind for a
different meaning, changing endpoints for an existing edge family, or changing a
payload variant's semantics is breaking.

Enum payloads such as `NodeFact` and `EdgeFact` currently use serde's externally
tagged representation. Strict Rust deserializers will reject unknown variants.
Long-lived consumers that need forward compatibility should load through a
tolerant JSON layer or pin to supported `schema_version` values and fail with a
clear unsupported-kind diagnostic.

## Index Persistence

`ProgramSupergraph.indexes` is persisted in CLI JSON for convenience and
determinism, but indexes are derived from `nodes` and `edges`. Consumers may
discard and rebuild indexes during migration, after tolerant loading, or when
they suspect producer/consumer version skew.

Index entries must use stable subject IDs, never runtime graph handles such as
`petgraph::NodeIndex`, vector positions from another snapshot, or fact IDs.
`node_position_by_id` and `edge_position_by_id` are only valid for the serialized
`nodes` and `edges` vectors in the same snapshot.

`GraphIndexes.source_span_to_nodes` uses a JSON object, so its structured key is
serialized as a deterministic string:

```text
<artifact_id-or-empty>\u001f<start_byte>:<end_byte>:<start_row>:<start_column>:<end_row>:<end_column>
```

The separator is ASCII unit separator (`U+001F`). Empty artifact ID means the
span is not scoped to a known artifact. Consumers must parse all six span
numbers as unsigned byte/row/column positions and must not split on `:` before
separating the artifact prefix. Changing this encoding is breaking for
`program-supergraph.v2`.

## Provenance And Evidence

Persisted graph facts should carry enough provenance to explain and refresh
derived facts without repeating graph-level metadata:

- top-level graph language;
- optional artifact `content_hash`;
- evidence content hash, source ID, source span, and syntax reference where
  available;
- source ownership for artifact, scope, and callable;
- confidence and uncertainty classifications.

`content_hash` and `payload_hash` have different jobs. Content hashes describe
source artifacts. Payload hashes identify exact serialized fact payloads.
Consumers should not substitute one for another during migration.

## Confidence And Uncertainty

`confidence` records strength of the producer's claim. `uncertainty` records the
status category of the fact: exact, probable, possible, external, unresolved,
ambiguous, stale, or unsupported.

When migrating older snapshots, preserve explicit uncertainty if present. If it
is missing, derive it conservatively from resolution, diagnostic kind, domain
knowledge status, precision strings, or confidence using the same refresh rules
as the producer. Consumers should expose uncertain, unresolved, ambiguous,
external, stale, and unsupported facts instead of silently dropping them.

## Domain Knowledge

`DomainKnowledge` nodes and `DependsOnDomainKnowledge` edges are prose context
only. They may improve generated titles and summaries, and stale domain knowledge
may produce diagnostics, but they are not deterministic code evidence.

Consumers must not treat domain knowledge as a substitute for `TracesTo` code
facts, source spans, parser evidence, or analysis-derived control/data/call
facts. Migration code should preserve domain facts separately from code-backed
trace evidence.

## Trace And Invalidation Expectations

`TracesTo` edges connect generated requirements to code-backed graph facts.
Invalidation is a derived view concern, not a public JSON edge family. Consumers
should derive dirty subjects from stable subject IDs in ownership, source-span,
provenance, `TracesTo`, `DependsOnDomainKnowledge`, and existing semantic edges.

If a migrated snapshot rebuilds facts and changes `fact_id` values while
preserving subject IDs, derived invalidation closures should continue to point at
the same subjects unless the underlying code evidence, requirement evidence, or
dependency relationship actually changed.

## CLI JSON Compatibility

The public supergraph CLI commands emit the full `ProgramSupergraph` snapshot,
including indexes, using the same schema as library callers. Pretty and compact
JSON must be semantically equivalent. Output ordering is deterministic for a
given input, language, and source content.

CLI consumers should:

- check `schema_version` before reading the body;
- tolerate additive fields they do not use;
- rebuild indexes when migrating or normalizing snapshots;
- compare stable subject IDs for graph references and `payload_hash`/`fact_id`
  for exact fact-version comparisons;
- include fixture snapshots or roundtrip tests for every schema version they
  claim to support.

## Recommended Consumer Tests

Downstream consumers should keep lightweight compatibility tests that:

- deserialize the newest supported `ProgramSupergraph` JSON;
- reject unknown `schema_version` values with a clear error;
- roundtrip `SourceSpanIndexKey` strings and rebuild `GraphIndexes`;
- load older snapshots that omit defaulted fields and verify refresh/backfill
  behavior;
- compare subject IDs separately from `fact_id` and `payload_hash`;
- preserve uncertain, unsupported, external, unresolved, stale, ambiguous, and
  domain-knowledge facts in user-visible diagnostics or metadata.
