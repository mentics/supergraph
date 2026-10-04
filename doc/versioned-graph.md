# Versioned Graph Design Note

We don't know if we're building this yet, but I want to track this conversation
and the information we got.

This document records how the program supergraph could support efficient
versioning and graph diffs if the project needs to preserve historical graph
versions.

## Core Idea

The runtime program supergraph should remain a traversal-optimized in-memory
graph. The durable source of truth should be a versioned fact store.

Do not persist a complete serialized graph snapshot for every version. Instead,
persist immutable graph facts and store which facts belong to each graph version.
Runtime graphs can then be materialized from a selected version.

The architecture becomes:

```text
parsed artifacts
  -> normalized graph facts
  -> versioned fact store
  -> materialized runtime supergraph
  -> logical views, requirements, docs, and sync
```

## Subject IDs Versus Fact IDs

Efficient versioning requires two different identities:

- `subject_id`: stable identity for the conceptual thing across versions.
- `fact_id`: immutable identity for one exact version of that thing's payload.

For example, a callable has a stable subject:

```text
subject_id = callable:python:app.risk.calculate_risk
```

Each version of that callable's facts has a separate fact ID:

```text
calculate_risk@hash_a
calculate_risk@hash_b
```

The subject stays the same when the function body changes. The fact changes
because its payload changed.

## Why Calls Should Reference Subjects

Persistent semantic references should usually point to subject IDs, not fact
IDs.

If a caller points directly to a callee's fact ID, then changing the callee body
creates a new callee fact and can make unchanged callers point at an obsolete
fact. That is the wrong behavior.

Instead, a call edge should refer to the callee subject:

```text
CallEdgeFact:
  edge_subject_id
  caller_subject_id
  call_site_subject_id
  callee_subject_id
  resolution
  evidence
  payload_hash
```

When materializing a graph version, the loader resolves each subject reference to
the fact that is current for that subject in that version.

Example:

```text
Version 1:
  create_incident -> callable_fact_a1
  calculate_risk -> callable_fact_r1
  call(create_incident, calculate_risk) -> call_edge_fact_c1

Version 2:
  create_incident -> callable_fact_a1
  calculate_risk -> callable_fact_r2
  call(create_incident, calculate_risk) -> call_edge_fact_c1
```

The caller did not change, and the call expression still resolves to the same
callee subject, so the call edge fact can be reused. When version 2 is loaded,
`calculate_risk` resolves to `callable_fact_r2`.

If the call expression, resolution, argument compatibility, or evidence changes,
the call edge can keep the same edge subject while receiving a new fact ID.

## Stable Subject Identity

A subject ID can be derived from a fully qualified name or from a hash of a
canonical identity tuple.

For callables, a reasonable identity tuple is:

```text
language
repository or package root
module path
lexical owner
callable name
callable kind
overload or signature discriminator when needed
```

Do not include body text, body hash, implementation details, or ordinary body
spans in the subject ID. Those belong in versioned facts.

If the fully qualified name changes, the callable may become a different
subject. The system can still detect renames as a higher-level operation, but it
should not silently pretend that unrelated subjects are the same.

Examples of subject changes:

- function renamed without continuity evidence
- function moved to a different module or lexical owner
- overload discriminator changed
- call site moved to a different enclosing callable
- call resolution changed to a different target

When continuity is uncertain, emit an identity churn diagnostic instead of
guessing.

## Version Records

A graph version identifies the current fact for each subject in that version.

Possible persisted records:

```text
GraphFact:
  fact_id
  subject_id
  fact_kind
  payload_hash
  payload
  evidence
  owner_artifact_id
  owner_callable_id
  analysis_phase
  analysis_version

GraphVersion:
  version_id
  parent_version_id
  repository_revision
  root_fact_set_id
  created_at

VersionMembership:
  version_id
  subject_id
  fact_id
```

For small repositories, explicit version membership may be acceptable. For larger
histories, use structural sharing.

## Efficient Storage Options

The store should avoid duplicating unchanged facts across versions.

Good options:

- Delta chain: each version stores added, removed, and replaced fact IDs relative
  to its parent.
- Persistent set: each version root points to an immutable set of subject-to-fact
  mappings that shares structure with previous versions.
- Merkle tree: each version root is a hash tree over sorted subject-to-fact
  mappings; unchanged subtrees are reused.
- Chunked sorted lists: mappings are grouped by artifact, fact kind, or hash
  prefix so diffs can skip unchanged chunks.
- Periodic snapshots plus deltas: full materialized indexes every N versions,
  with parent-relative deltas between snapshots.

A Merkle or persistent-set approach is attractive because a new version only
stores changed chunks and new facts, while unchanged graph areas are shared.

## Efficient Diffs

Graph diffs should be computed from fact sets, not by running a generic graph
edit-distance algorithm.

The basic diff is:

```text
removed = old_version.subject_map - new_version.subject_map
added = new_version.subject_map - old_version.subject_map
changed = same subject_id, different fact_id or payload_hash
unchanged = same subject_id, same fact_id
```

Then group changes by:

- artifact
- callable
- requirement
- fact kind
- edge kind
- source span
- analysis phase
- impacted graph slice

For Merkle-backed maps, diff can skip entire unchanged subtrees by comparing
subtree hashes.

## Incremental Update Flow

When code changes:

1. Hash changed artifacts.
2. Reparse only changed artifacts.
3. Recompute facts owned by those artifacts.
4. Query the derived invalidation view to recompute dependent facts, such as
   imports, symbol resolution, call edges, CFG, DFG, requirements, and trace
   links.
5. Compare newly produced facts with the previous version's current facts.
6. Write new facts only when payloads changed.
7. Create a new graph version that reuses unchanged subject-to-fact mappings.
8. Materialize or patch the runtime supergraph for the new version.

Runtime graph mutation is an optimization. Versioned facts are the durable source
of truth.

## Runtime Materialization

When loading a version, the runtime graph maps stable subjects to the selected
version's facts.

```text
subject_id -> current fact_id in version
fact_id -> runtime node or edge payload
runtime NodeIndex/EdgeIndex -> subject_id and fact_id
```

Traversal uses runtime indexes, but externally visible references should resolve
through stable IDs.

For example, an unchanged call edge can still point to the same callee subject
even when the callee's implementation fact changed. The version loader connects
that edge to the current callee fact for the selected version.

## Requirement Sync Benefits

Versioned facts make requirements-to-code and code-to-requirements sync easier to
explain.

If a requirement changes batch size from `25` to `30`, the next version can show:

- the leaf requirement fact changed
- the literal value fact changed
- the affected code artifact hash changed
- the trace link stayed the same if it still points to the same subject/span
- higher-level requirement subjects stayed the same
- unrelated call, control, and data-flow facts were reused

The diff can explain:

```text
Requirement R changed literal value 25 -> 30.
The change traces to code span S in callable C.
No boundary-level requirement structure changed.
```

## Open Design Questions

- Which storage backend should hold versioned facts: SQLite, `redb`, content
  addressed files, or a hybrid?
- Should the first implementation use explicit version membership before moving
  to a Merkle-backed map?
- How often should full materialized snapshots be stored?
- How should rename detection preserve subject continuity?
- What is the exact identity tuple for each subject kind?
- Which facts are owned by an artifact versus by cross-artifact resolution?
- How should generated requirement prose be versioned relative to requirement
  graph facts?
