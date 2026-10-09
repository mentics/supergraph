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
current version is `program-supergraph.v3`. Loaders accept only that version;
any other `schema_version` is rejected with an error. There is no migration path
from earlier versions: regenerate the snapshot.

### v3 format

- `indexes` is not written. Indexes are derived data and are rebuilt when a
  snapshot is loaded.
- Interned strings (names, kinds, paths, text fields typed as interned symbols in
  the Rust schema) are written through a top-level `strings` array placed before
  `nodes`. Each such field holds an integer position into `strings` instead of
  the text. The table is sorted by text so output stays deterministic. A reader
  must parse `strings` before `nodes` and `edges`; the writer always emits it
  first.
- `build --expand-strings` (and `ProgramSupergraph::write_json_expanded`) writes
  the same schema with every string inline and no `strings` member. Readers accept
  both forms: a field that is a JSON string is taken as text, a number as a table
  position. `--pretty` output is always expanded.
- Node ids are `prefix:hex16` hashes. Callable and external-name ids are hashes
  like every other id (they no longer carry readable names); ids derived from
  them, `fact_id` and `payload_hash` hash those ids normally. Payload hashes are
  computed over the expanded text, so they do not depend on which form was
  written.
- `SourceSpan` byte, row and column positions are 32-bit unsigned values.

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

`fact_id` and `payload_hash` are optional on read (absent means unknown) and are
recomputed by the normal graph refresh pipeline before persistence.

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

`ProgramSupergraph.indexes` is not persisted. Indexes are derived from `nodes` and
`edges` and are rebuilt by the Rust loader; other consumers should build their own
from stable subject IDs, never from runtime graph handles or vector positions of
another snapshot.

`SourceSpanIndexKey` (artifact id plus six span numbers) is an in-memory index key
only; it has no persisted encoding.

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

The public supergraph CLI commands emit the full `ProgramSupergraph` snapshot
(nodes and edges, without indexes) using the same schema as library callers. Pretty and compact
JSON must be semantically equivalent. Output ordering is deterministic for a
given input, language, and source content.

CLI consumers should:

- check `schema_version` before reading the body;
- tolerate additive fields they do not use;
- build their own indexes from nodes and edges;
- compare stable subject IDs for graph references and `payload_hash`/`fact_id`
  for exact fact-version comparisons;
- include fixture snapshots or roundtrip tests for every schema version they
  claim to support.

## Recommended Consumer Tests

Downstream consumers should keep lightweight compatibility tests that:

- deserialize the newest supported `ProgramSupergraph` JSON;
- reject unknown `schema_version` values with a clear error;
- rebuild indexes after loading;
- compare subject IDs separately from `fact_id` and `payload_hash`;
- preserve uncertain, unsupported, external, unresolved, stale, ambiguous, and
  domain-knowledge facts in user-visible diagnostics or metadata.
