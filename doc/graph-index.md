# Graph Index Design

This document describes the runtime graph index used for traversal-heavy
analysis. The graph index is separate from durable persistence. Persistent
stores keep facts, versions, provenance, and dirty state; the graph index is the
in-memory execution format used by logical supergraph views for calls,
data-flow, impact, traceability, and sync analysis.

## Goals

- Traverse call, data-flow, requirement, and dependency edges quickly.
- Preserve stable identities from persisted facts without making runtime graph
  indexes part of the durable data model.
- Support graph algorithms needed by local analysis, such as reachability,
  reverse impact, strongly connected components, cycle handling, and path
  discovery.
- Keep the traversal implementation swappable if profiling later shows that a
  more compact graph layout is needed.

## Suggested Uses Of `petgraph`

Use `petgraph` for runtime analysis that needs to walk relationships across the
facts produced by the AST-to-supergraph pipeline, `CallGraphView`, data-flow
views, requirements graph, and sync engine. It should be used where the question
is naturally answered by graph traversal, path discovery, reachability, or
strongly connected component analysis.

Specific situations where `petgraph` is a good fit:

- **Entrypoint discovery.** Use `CallGraphView` over `ProgramSupergraph` to walk
  resolved local `Calls` facts, then find callables with no incoming local calls.
  These candidates feed the requirements layer's `entrypoint` requirement model.
- **Boundary reachability.** Starting from known or candidate boundary callables,
  traverse outgoing call edges to identify the implementation slice behind a
  business-facing behavior.
- **Reverse impact analysis.** When a callable, source artifact, schema, or test
  changes, walk incoming call, data-flow, and traceability edges to find affected
  entrypoints, boundary requirements, generated requirement documents, and domain
  knowledge records.
- **Requirement coverage checks.** Traverse requirement-to-code trace edges and
  code-to-requirement reverse edges to identify dev artifacts that are not
  represented by requirements and requirements that no longer have supporting
  implementation evidence.
- **Requirement decomposition traversal.** Walk from boundary requirements
  through functional, decomposition, structural, and leaf requirements to explain
  how a high-level behavior is implemented by exact code-backed facts.
- **Bidirectional sync planning.** Given a code edit or requirement edit, walk
  the dependency graph to decide which graph slices must be recomputed, which
  requirements are stale, and which generated outputs need to be refreshed before
  the system is `at rest`.
- **Domain knowledge staleness detection.** Connect domain knowledge records to
  the artifacts, callables, spans, and requirements they explain. On source
  changes, traverse those dependencies to report missing or stale domain
  knowledge before applying it to future modifications.
- **Data-flow path discovery.** Build data-flow edges between definitions,
  assignments, parameters, returns, call arguments, call results, fields, and
  sinks. Use traversal to answer whether a value can flow from a source to a
  sink, including through calls.
- **Call-plus-data-flow analysis.** Combine call edges and data-flow edges when
  the question crosses abstraction layers, such as "which entrypoints can pass
  user input into this persistence write or external API call?"
- **Cycle and strongly connected component detection.** Detect recursive call
  groups, import cycles, mutually dependent requirement references, and cyclic
  data-flow relationships so downstream analysis can handle them explicitly.
- **Uncertainty-aware traversal.** Include edge metadata for `exact`,
  `probable`, `possible`, `external`, and `unresolved` relationships so callers
  can choose whether impact analysis should follow only deterministic edges or
  also include uncertain paths.
- **Path explanation for users and agents.** When reporting why a requirement is
  impacted or why an artifact is uncovered, find representative paths through
  the graph and then resolve those runtime nodes back to stable IDs, source
  spans, and evidence in the persistent store.
- **Incremental invalidation.** Maintain a dependency graph from artifacts to
  parser outputs, lowered semantic facts, symbol tables, call edges, data-flow
  edges, requirements, and domain knowledge. Traverse it after file changes to
  choose the smallest safe recomputation set.

## Non-Goals

- Use a database as the inner traversal engine.
- Make `petgraph` node indexes stable across runs.
- Store raw ASTs or large serialized payloads inside the runtime graph.
- Optimize prematurely for graphs far larger than the repositories the project
  needs to support initially. Initial support is for 1000's of code files.

## Core Decision

Use `petgraph` as the first in-memory graph traversal layer.

`petgraph` gives the project a mature Rust graph implementation with built-in
support for common traversal and analysis operations. It is a good fit while the
project is still discovering its exact graph queries because it lets the engine
focus on correctness, identity, and invalidation before committing to a custom
adjacency layout.

The storage layer remains independent. SQLite, `redb`, JSON files, or a hybrid
store may persist the underlying facts, but traversal should happen over a
loaded graph index rather than repeated database lookups.

## Persistence Versus Traversal

The persistent store answers questions such as:

- Which artifacts, callables, call sites, requirements, and trace links exist?
- Which content hash and parser version produced this fact?
- Which graph slices are dirty after a file change?
- Which version root or analysis snapshot does this fact belong to?
- Which spans and evidence explain this edge?

The graph index answers questions such as:

- Which callables are reachable from this entrypoint?
- Which requirements are impacted by this changed callable?
- Which data-flow paths reach a sink?
- Which nodes participate in a cycle or strongly connected component?
- Which unresolved calls sit on paths relevant to a requirement?

Databases can store edges, but compiler-style static analysis should traverse
tight in-memory adjacency structures. This avoids turning graph algorithms into
large numbers of small database reads.

## Runtime Identity

Persisted IDs and runtime graph indexes serve different purposes.

Persistent records should use stable IDs such as:

- `artifact_id`
- `callable_id`
- `call_site_id`
- `requirement_id`
- `edge_id`

The runtime graph should map those IDs to `petgraph` indexes:

```rust
use std::collections::HashMap;

use petgraph::graph::{Graph, NodeIndex};

struct RuntimeGraph {
    graph: Graph<Node, Edge>,
    stable_to_node: HashMap<StableId, NodeIndex>,
    node_to_stable: Vec<StableId>,
}
```

`NodeIndex` is an ephemeral handle. It must not be written to disk, exposed as a
stable API identity, or used in generated requirements. Rebuild the mapping when
loading persisted graph facts.

## Node And Edge Shape

The graph should store compact runtime nodes and edges. Large evidence payloads,
AST fragments, requirement text, and source content should stay in the
persistent store or content-addressed cache.

Typical node kinds:

- Artifact
- Callable
- Scope
- Symbol
- Requirement
- Domain knowledge record
- External target
- Data-flow location

Typical edge kinds:

- Calls
- Imports
- Binds
- Contains
- DataFlow
- DecomposesTo
- Conditions
- Orders
- TracesTo
- Requires
- DependsOnDomainKnowledge

Edges should carry enough information for traversal filtering:

```rust
enum EdgeKind {
    Calls,
    Imports,
    Binds,
    Contains,
    DataFlow,
    DecomposesTo,
    Conditions,
    Orders,
    TracesTo,
    Requires,
    DependsOnDomainKnowledge,
}

struct Edge {
    kind: EdgeKind,
    confidence: Confidence,
    evidence_id: Option<EvidenceId>,
}
```

The `evidence_id` should point back to persisted spans, resolution evidence, or
diagnostics instead of embedding those details in the graph.

Incremental invalidation should be exposed as a derived runtime view or index
over canonical edges, ownership, spans, provenance, traces, and dependencies
rather than as a persisted public edge kind.

## Graph Layers

See `doc/supergraph.md` for the full program supergraph architecture. This
section describes the runtime traversal consequences of that design.

The architecture uses one canonical `ProgramSupergraph` with typed edges and
traversal filters. The runtime API exposes focused logical views so callers do
not need to know whether a query is backed by the physical graph vectors or by
specialized indexes derived from them:

- Walk only call edges.
- Walk only data-flow edges.
- Walk call and data-flow edges.
- Walk reverse traceability edges.
- Walk dependency edges for invalidation.

If profiling later shows that a single physical runtime graph is confusing or
slow, keep `ProgramSupergraph` as the canonical model and add focused derived
indexes. Call traversal should continue to use the same naming split as
`GraphIndexes`: all-call caller/call-site indexes such as `calls_by_caller` and
`call_site_to_calls`, and concrete-target indexes such as
`calls_by_concrete_target`, `caller_to_concrete_target_calls`, and
`caller_to_concrete_call_targets`.

Other view-specific indexes can follow the same pattern:

- `DataFlowGraphIndex`
- `RequirementTraceIndex`
- `InvalidationIndex`
- `ImpactGraphIndex`

The public API should avoid exposing this internal storage choice.

## Incremental Updates

The graph index should be rebuildable from persisted facts, but normal editor
feedback should avoid full rebuilds when possible.

When a file changes:

1. Recompute facts for the changed artifact.
2. Identify graph nodes and edges owned by that artifact.
3. Remove or replace affected nodes and edges.
4. Re-resolve dependent imports, bindings, call sites, and data-flow facts.
5. Update incoming and outgoing traversal indexes.
6. Recompute derived summaries such as entrypoint status, incoming edge counts,
   stale requirements, and impacted domain knowledge records.

If incremental mutation becomes too complex, prefer rebuilding a focused graph
slice or the full in-memory graph before accepting subtle identity bugs. For
repositories with thousands of files, full graph rebuilds may still be cheap
enough during early development.

## Performance Expectations

For large-ish local repositories, `petgraph` should be sufficient as a starting
point. Tens of thousands of callables and hundreds of thousands of edges are
reasonable if node payloads are compact and large evidence is stored elsewhere.

The likely expensive phases are parsing, symbol resolution, invalidation, and
data-flow construction. Traversing adjacency lists should not be the first
bottleneck.

If profiling later shows graph traversal or memory layout is limiting, replace
the internal graph storage with a compact adjacency representation such as
compressed sparse row:

```text
nodes: Vec<Node>
edges: Vec<Edge>
out_offsets: Vec<u32>
out_edges: Vec<EdgeIndex>
in_offsets: Vec<u32>
in_edges: Vec<EdgeIndex>
```

The project should hide `petgraph` behind its own graph index API so this change
does not affect the rest of the engine.

## Gotchas

- Do not persist `NodeIndex` or `EdgeIndex`; they are runtime-only.
- Do not perform traversal by repeatedly querying SQLite or `redb`.
- Do not store full ASTs, source text, or large evidence blobs in graph nodes.
- Do not assume one edge type can answer every analysis question without
  filtering.
- Do not hide unresolved or uncertain edges. Traversal should be able to include
  or exclude uncertain paths explicitly.
- Do not let graph database features drive the core design. This project is
  closer to a compiler/indexer than an ad hoc graph-query product.

## Recommended Initial API

Start with a small project-owned abstraction:

```rust
struct GraphIndex {
    runtime: RuntimeGraph,
}

impl GraphIndex {
    fn node_for_stable_id(&self, id: &StableId) -> Option<NodeIndex>;
    fn stable_id_for_node(&self, node: NodeIndex) -> Option<&StableId>;
    fn reachable(
        &self,
        start: StableId,
        direction: Direction,
        edge_filter: EdgeFilter,
    ) -> Vec<StableId>;
}
```

The API should return stable IDs, not `petgraph` indexes. Callers can then fetch
details, spans, diagnostics, and requirement text from the persistent store.

## Summary

Use `petgraph` for the first in-memory traversal implementation. Keep stable
identity and persistence independent from runtime graph handles. Store facts and
evidence durably, load compact graph nodes and typed edges for analysis, and
hide the implementation behind a project-owned graph index API so the internal
layout can evolve later.
