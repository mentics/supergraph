# Supergraph-Only Cleanup Plan

This document describes how to remove the separate `CallGraph` implementation
and make `ProgramSupergraph` the only graph built by the analysis pipeline.

The desired final state is intentionally clean: a reviewer should see one graph
model, one builder path, one indexing strategy, and graph-specific queries
implemented as views over `ProgramSupergraph`. The code should not look like a
supergraph layer wrapped around an older call graph implementation.

## Target Outcome

The final analysis pipeline should be:

1. Discover source files.
2. Parse each file into `ProjectAst`.
3. Lower language-specific AST facts directly into `ProgramSupergraphBuilder`.
4. Resolve calls, symbols, scopes, bindings, external targets, and diagnostics
   directly as supergraph nodes and edges.
5. Run semantic enrichment over the resulting `ProgramSupergraph`.
6. Expose call graph, CFG, DFG, PDG, requirements, traceability, and
   invalidation as views over the same physical graph.
7. Serialize only `ProjectAst` and `ProgramSupergraph` from first-class CLI/API
   entry points.

There should be no standalone `CallGraph` schema, no `call_graph_to_supergraph`
conversion, no `call_graph_from_supergraph` compatibility projection, and no
`call-graph-*` CLI commands unless a deliberate external compatibility contract
is reintroduced later.

## Non-Goals

- Do not redesign the parser or `ProjectAst` in this cleanup.
- Do not change the semantic meaning of existing call resolution, import
  resolution, or external target detection.
- Do not change stable IDs unless a direct supergraph ID is currently wrong or
  ambiguous.
- Do not introduce a database or persistence layer.
- Do not broaden language support.
- Do not rewrite CFG, DFG, requirements, sync, or invalidation logic except where
  they depend on removed call graph compatibility APIs.

## Final Architecture

### Modules

The final module layout should make `ProgramSupergraph` the canonical model:

- `src/supergraph/schema.rs`: all physical graph node, edge, fact, and index
  types.
- `src/supergraph/builder.rs`: direct construction helpers, stable ID helpers,
  sorting, provenance refresh, fact identity refresh, and index construction.
- `src/supergraph/views.rs`: read-only logical graph views over
  `ProgramSupergraph`.
- `src/analysis.rs`: public analysis entry points that return `ProjectAst` or
  `ProgramSupergraph`.
- `src/analysis/calls.rs`: call-specific enrichment and call view support over
  supergraph facts.
- `src/analysis/lowering.rs` or `src/analysis/source_graph.rs`: the new direct
  AST-to-supergraph lowering pass.
- `src/analysis/language.rs` or `src/analysis/adapters.rs`: language-specific
  adapter behavior currently held in `src/call_graph/adapter.rs`,
  `src/call_graph/python.rs`, and `src/call_graph/typescript.rs`.

After cleanup, there should be no `src/call_graph.rs` and no
`src/call_graph/` directory. If a file still needs call-specific helper names,
the names should describe what the code does now, such as `CallResolution`,
`CallResolver`, `CallGraphView`, or `CallViewFilter`, not `CallGraph` as a
physical data model.

### Public API

Keep these first-class APIs:

- `analyze_python_path(path) -> Result<ProjectAst>`
- `analyze_typescript_path(path) -> Result<ProjectAst>`
- `analyze_python_supergraph(path) -> Result<ProgramSupergraph>`
- `analyze_typescript_supergraph(path) -> Result<ProgramSupergraph>`

Remove these APIs:

- `analyze_python_call_graph(path) -> Result<CallGraph>`
- `analyze_typescript_call_graph(path) -> Result<CallGraph>`
- `build_python_call_graph(project) -> CallGraph`
- `build_typescript_call_graph(project) -> CallGraph`
- `call_graph_to_supergraph(graph) -> ProgramSupergraph`
- `call_graph_from_supergraph(graph) -> CallGraph`

If an external caller needs call-oriented traversal, expose a view:

- `ProgramSupergraph::call_graph_view() -> CallGraphView`
- `CallGraphView::call_edges(filter)`
- `CallGraphView::callables()`
- `CallGraphView::call_sites()`
- `CallGraphView::external_targets()`
- indexed helpers such as `calls_from_call_site`,
  `calls_from_caller_to_target`, and `call_targets_from_caller`

The view should return `GraphNode`, `GraphEdge`, or typed supergraph fact
references. It should not allocate or return a separate `CallGraph` object.

### CLI

Keep:

- `analyze-python`
- `analyze-typescript`
- `supergraph-python`
- `supergraph-typescript`

Remove:

- `call-graph-python`
- `call-graph-typescript`

If call graph JSON is still useful for humans, add it later as a filtered
supergraph output command with supergraph terminology, for example
`supergraph-python --view calls`. Do not keep the old command names during this
cleanup, because the goal is to remove historical ambiguity.

## Current State To Replace

The current implementation has three overlapping layers:

- `ProjectAst` in `src/ast.rs`.
- A standalone `CallGraph` schema in `src/call_graph/schema.rs`.
- `ProgramSupergraph` in `src/supergraph/schema.rs`.

The current supergraph path still passes through call graph-era code:

1. `analyze_python_supergraph` and `analyze_typescript_supergraph` parse a
   `ProjectAst`.
2. `build_python_supergraph` and `build_typescript_supergraph` call
   `build_supergraph` in `src/call_graph/core.rs`.
3. `GraphBuilder` stores artifacts, scopes, bindings, callables, call sites,
   call edges, diagnostics, and resolution context in call graph-shaped
   collections.
4. `GraphBuilder::finish` creates a temporary `CallGraph`.
5. `call_graph_to_supergraph` converts that `CallGraph` to
   `ProgramSupergraph`.
6. `enrich_supergraph_with_semantic_flows` adds the newer supergraph-only facts.

The cleanup must preserve the useful behavior in this pipeline while removing
the standalone model and conversion step.

## Guiding Rules

- `ProgramSupergraph` is the only physical graph.
- Every produced fact must be represented as `GraphNode` or `GraphEdge`.
- Logical graphs are filters and views, not separately materialized schemas.
- Temporary construction state is allowed, but it must not be named or shaped as
  a separate graph model.
- Language adapters should produce or resolve supergraph facts, not call graph
  records.
- Tests should assert supergraph facts and view behavior, not compatibility with
  removed schemas.
- Public docs should describe the current architecture, not a migration from an
  older call graph.
- Transitional code may exist within a short refactor series, but the final
  merged state should remove transitional names, compatibility projections, and
  old CLI/API surfaces.

## Phase 0: Baseline And Safety Net

### SG-CLEAN-000: Record Current Behavior

Deliver:

- Run and record the current test baseline:
  - `cargo test`
  - targeted e2e tests for Python and TypeScript fixtures
  - CLI tests
- Capture representative `supergraph-python` and `supergraph-typescript` JSON
  snapshots for the existing example fixtures if useful for local comparison.

Done when:

- Current behavior is known before refactoring.
- Any pre-existing failures are documented separately from cleanup failures.

### SG-CLEAN-001: Define No-Regression Checks

Deliver:

- A checklist of facts that must remain present after direct supergraph lowering:
  - artifacts
  - module, class, and function scopes
  - bindings and `Binds` edges
  - callables, including module initializers and implicit constructors
  - call sites
  - local, external, and unresolved `Calls` edges
  - external targets
  - diagnostics
  - exact, external, possible dynamic, and unresolved resolution metadata
  - caller/callee/call-site indexes
  - incoming local call counts
  - decorated callable external invocation metadata

Done when:

- These checks exist as tests or are covered by existing tests that will be
  rewritten in later phases.

## Phase 1: Introduce Direct Supergraph Lowering

### SG-CLEAN-010: Create A Direct Lowering Module

Deliver:

- Add a new internal module for initial AST-to-supergraph construction, for
  example `src/analysis/source_graph.rs`.
- Move the conceptual responsibilities of `src/call_graph/core.rs` into this
  module, but do not copy the standalone `CallGraph` shape.
- Introduce a builder state with a name such as `SourceGraphLowerer` or
  `InitialSupergraphLowerer`.

The lowerer may keep temporary state:

- file contexts
- local module names
- local callable IDs
- class summaries
- short class name indexes
- pending calls
- pending imports or assignments
- external target deduplication

The lowerer must not keep final graph facts in fields named `artifacts`,
`scopes`, `bindings`, `callables`, `call_sites`, or `call_edges` as independent
graph-shaped vectors. Instead, insert final facts into `ProgramSupergraphBuilder`
as soon as their IDs and owners are known.

Done when:

- There is a compileable internal module that can own direct construction.
- No public behavior has changed yet.

### SG-CLEAN-011: Define A Supergraph Adapter Trait

Deliver:

- Replace `CallGraphAdapter` with a trait named for the new role, for example
  `LanguageSupergraphAdapter` or `SourceGraphAdapter`.
- Keep language-specific responsibilities that are still needed:
  - language identifier
  - analysis version
  - module path normalization
  - import normalization
  - constructor naming
  - callable kind mapping
  - dispatch kind mapping
  - call edge kind mapping
  - external target construction
  - imported-name target resolution
  - assignment-to-class detection
  - assignment-to-external detection
  - field flow population
  - call resolution

Clean up naming:

- Rename `ResolvedCall` variants to use supergraph concepts:
  - `LocalTarget`
  - `ExternalTarget`
  - `UnresolvedTarget`
- Replace call graph enum imports with `supergraph::CallEdgeKind`,
  `supergraph::Confidence`, `supergraph::Resolution`, and
  `supergraph::ExternalTargetKind`.
- Keep temporary helper structs such as `ClassInfo`, `FileContext`,
  `ImportBinding`, and `PendingCall` only if their names match their role.

Done when:

- Python and TypeScript adapters compile against supergraph schema types.
- No code outside the lowering module needs `src/call_graph/schema.rs`.

### SG-CLEAN-012: Insert Base Nodes Directly

Deliver:

- Update file lowering to call `ProgramSupergraphBuilder` directly for:
  - `add_artifact`
  - `add_scope`
  - `add_callable`
  - `add_binding`
  - `add_contains`
  - `add_binds`
  - `add_resolves_to_with_resolution`
  - `add_diagnostic`

Preserve semantics:

- module initializer callable per source file
- module scope ownership
- class scopes
- implicit constructors
- explicit constructors
- function and method scopes
- class method bindings
- module function bindings
- parameter bindings
- assignment bindings
- import bindings

Done when:

- The base supergraph contains the same artifact, scope, binding, callable,
  diagnostic, `Contains`, `Binds`, and `ResolvesTo` facts as before.
- The old `call_graph_to_supergraph` conversion is no longer needed for these
  fact families.

### SG-CLEAN-013: Resolve Calls Directly Into Supergraph Edges

Deliver:

- Update pending call resolution to insert:
  - `CallSite` nodes
  - call-site containment edges
  - external target nodes
  - `Calls` edges
  - unresolved-call diagnostics

Use `ProgramSupergraphBuilder::add_calls` or a small helper around it. The
helper should set:

- source node: call site ID
- target node: callee callable ID or external target ID
- owner: call artifact and caller callable
- span: call site span
- confidence: derived from resolution
- uncertainty: derived by builder refresh logic
- evidence: parser or resolver evidence as structured supergraph `Evidence`
- fact payload: `EdgeFact::Calls`

Preserve existing behavior:

- local direct calls
- local method calls
- constructor calls
- decorator calls
- imported function calls
- imported class constructor calls
- module-object calls
- external library calls
- dynamic or unresolved calls
- unresolved target diagnostics

Done when:

- Supergraph e2e tests can assert all call behavior without constructing a
  standalone `CallGraph`.

### SG-CLEAN-014: Preserve Incoming Call Metadata Without A CallGraph Pass

Deliver:

- Replace the current `GraphBuilder::finish` incoming-call count pass with one of
  these clean approaches:
  - compute incoming counts before inserting callable nodes only if callables are
    inserted after resolution; or
  - add a focused post-processing function that mutates callable facts inside the
    initial supergraph before `finish`; or
  - represent incoming counts as a view/index property instead of a stored
    callable payload if no downstream code needs it serialized.

Recommendation:

- Prefer view/index-derived incoming counts unless the serialized
  `Callable::incoming_local_call_count` field is part of the supergraph contract.
- If the field stays, compute it as a named supergraph finalization step, not as
  part of a call graph conversion.

Done when:

- Decorated callable metadata and incoming local call counts remain correct.
- The implementation does not require a temporary `CallGraph`.

### SG-CLEAN-015: Finish Direct Supergraph Construction

Deliver:

- The direct lowerer returns `ProgramSupergraphBuilder::finish()`.
- `build_python_supergraph(project)` and `build_typescript_supergraph(project)`
  call the direct lowerer.
- `analysis::enrich_supergraph_with_semantic_flows` remains the next stage.

Done when:

- `analyze_python_supergraph` and `analyze_typescript_supergraph` no longer enter
  `src/call_graph/core.rs`.
- Supergraph JSON output remains deterministic.
- Supergraph indexes are populated by the canonical builder/index path.

### SG-CLEAN-016: Complete The Typed Builder API

Deliver:

- Add typed `ProgramSupergraphBuilder` helpers for edge families that are still
  commonly emitted through raw `insert_edge` or local ad hoc helpers:
  - `ControlFlow`
  - `Controls`
  - `Defines`
  - `Uses`
  - `DataFlow`
  - `ParameterIn`
  - `ReturnsTo`
  - `ParameterOut`
  - `ThrowsTo`
  - `DecomposesTo`
  - `Conditions`
  - `Orders`
  - `TracesTo`
  - `DependsOnDomainKnowledge`
- Keep `insert_edge` available as a low-level escape hatch, but make normal
  analysis code read as typed supergraph construction.
- Use the same conventions as existing `add_contains`, `add_binds`,
  `add_resolves_to_with_resolution`, and `add_calls`:
  - stable edge ID from the fact family and endpoint IDs
  - canonical `source_id` and `target_id`
  - explicit owner and span
  - structured evidence
  - confidence and uncertainty refreshed through builder finalization

Done when:

- New direct lowering and semantic enrichment code can construct graph facts
  without duplicating edge-construction boilerplate.
- A reviewer sees one coherent builder API instead of a mix of legacy conversion
  helpers and local graph-edge assembly.

## Phase 2: Rewrite Tests Around Supergraph Views

### SG-CLEAN-020: Replace CallGraph Fixture Tests

Deliver:

- Rewrite Python and TypeScript tests named like
  `analyzes_*_fixture_into_call_graph`.
- They should call `analyze_*_supergraph`, create `CallGraphView`, and
  assert through view helpers or direct `EdgeFact::Calls` facts.

Replace helpers:

- `assert_local_edge(&CallGraph, caller, callee)` becomes
  `assert_local_call(&ProgramSupergraph, caller, callee)`.
- `assert_external_edge(&CallGraph, caller, external)` becomes
  `assert_external_call(&ProgramSupergraph, caller, external)`.
- `callable_id(&CallGraph, qualified_name)` becomes a supergraph node lookup.

Done when:

- No e2e test imports `req_graph::call_graph::CallGraph`.
- Call behavior is still covered for both languages.

### SG-CLEAN-021: Remove Compatibility Equality Tests

Deliver:

- Delete assertions comparing `call_graph_from_supergraph(&supergraph)` to
  `call_graph`.
- Replace `assert_equivalent_fact_counts` with direct supergraph shape checks.
- Replace `assert_equivalent_call_view` with view assertions that cover:
  - all call edge count
  - local call edge count
  - external call edge count
  - unresolved call edge count
  - caller-to-target helper behavior
  - call-site-to-call helper behavior

Done when:

- Tests verify the current architecture rather than preserving old compatibility.

### SG-CLEAN-022: Update CLI Tests

Deliver:

- Remove `call-graph-python` and `call-graph-typescript` command coverage.
- Ensure CLI tests cover:
  - `analyze-python`
  - `analyze-typescript`
  - `supergraph-python`
  - `supergraph-typescript`
  - unsupported file handling for supergraph commands
  - deterministic supergraph JSON output

Done when:

- CLI tests no longer expect standalone call graph JSON.

### SG-CLEAN-023: Update Internal Module Tests

Deliver:

- Update tests in `src/analysis/symbols.rs` and any other analysis module that
  imports `call_graph_from_supergraph`.
- Replace compatibility projections with:
  - `CallGraphView`
  - `ProgramSupergraph` node/edge queries
  - supergraph indexes

Done when:

- No test imports `call_graph_from_supergraph`.
- Internal tests still cover symbol resolution and call traversal.

## Phase 3: Remove Public Compatibility Surface

### SG-CLEAN-030: Remove Public CallGraph APIs

Deliver:

- Remove from `src/analysis.rs`:
  - `analyze_python_call_graph`
  - `analyze_typescript_call_graph`
- Remove from `src/lib.rs` re-exports:
  - `analyze_python_call_graph`
  - `analyze_typescript_call_graph`
- Remove from language modules:
  - `build_python_call_graph`
  - `build_typescript_call_graph`
- Remove public `pub mod call_graph` if no longer needed.

Done when:

- Public API exposes only AST and supergraph analysis.
- `cargo test` shows no downstream imports of removed APIs.

### SG-CLEAN-031: Remove CallGraph Schema

Deliver:

- Delete `src/call_graph/schema.rs`.
- Move any still-useful enum or payload variants into `src/supergraph/schema.rs`
  if they are not already present.
- Ensure all call-related code uses supergraph schema types:
  - `CallEdgeKind`
  - `Calls`
  - `CallSite`
  - `Callable`
  - `Binding`
  - `ExternalTarget`
  - `Resolution`
  - `Confidence`

Done when:

- No code refers to `req_graph::call_graph::schema`.
- No duplicate call-related schema remains.

### SG-CLEAN-032: Delete Conversion Functions

Deliver:

- Delete `call_graph_to_supergraph`.
- Delete `call_graph_from_supergraph`.
- Delete helper functions used only by those conversions:
  - call graph sort helpers
  - call graph index builders
  - call graph/supergraph enum conversion helpers
  - call graph evidence string conversion helpers
  - call graph binding/scope/callable conversion helpers

Keep only helpers that are useful to views or direct construction, and rename
them to supergraph terms.

Done when:

- `src/supergraph/views.rs` contains views only, not compatibility conversion.
- No function converts an entire graph into another graph model.

### SG-CLEAN-033: Remove CallGraph CLI Commands

Deliver:

- Delete `Command::CallGraphPython`.
- Delete `Command::CallGraphTypescript`.
- Delete branches in `run_with_args` that serialize standalone call graphs.
- Update CLI help text.

Done when:

- `req-graph --help` presents only AST and supergraph commands.
- There is no user-visible old graph command name.

### SG-CLEAN-034: Delete `src/call_graph`

Deliver:

- Move surviving adapter/lowering/resolution code into the new analysis modules.
- Delete `src/call_graph.rs`.
- Delete the `src/call_graph/` directory.
- Update `mod` declarations and imports.

Done when:

- `rg "call_graph" src tests` returns only intentionally named view terms such
  as `CallGraphView`, or no matches if views are renamed too.
- There is no standalone physical call graph implementation.

### SG-CLEAN-035: Remove Legacy Call Fallbacks

Deliver:

- Update `src/analysis/calls.rs` so it does not snapshot and preserve existing
  `EdgeFact::Calls` records as `legacy_calls`.
- Make call emission depend on supergraph-native facts only:
  - `CallSite` nodes
  - `ResolvesTo` edges from call sites to local callables or external targets
  - explicit unresolved-call facts or diagnostics for calls with no target node
- If unresolved calls need to survive without a target node, represent that
  behavior as a current supergraph invariant, not as a fallback to old call
  edges.
- Remove helper structs and maps that exist only to preserve older call edge
  payloads.

Done when:

- `analysis::calls::emit` has no `legacy` naming or compatibility branch.
- `Calls` edges are reproducible from canonical supergraph call-site resolution
  facts.
- Tests cover exact, external, possible dynamic, and unresolved calls without
  relying on pre-existing call edges.

## Phase 4: Clean Up Naming And Views

### SG-CLEAN-040: Finalize Call View Naming

Recommendation:

- Keep a call-oriented view, because call graph is still a useful logical graph.
- Rename it if the word `Current` reads like migration baggage. Good final names:
  - `CallGraphView`
  - `CallView`
  - `CallGraphFilter`
  - `CallViewFilter`

Avoid:

- migration or compatibility prefixes such as `Current`, `Compatibility`, or
  `Legacy` in view names

Done when:

- View names read like first-class logical views over the supergraph.
- No naming implies an older graph still exists.

### SG-CLEAN-041: Normalize Call Edge Payloads

Deliver:

- Review `EdgeFact::Calls` for fields that exist only to preserve old
  `CallEdge` compatibility.
- Keep denormalized fields only when they are genuinely useful for indexed,
  deterministic traversal.
- Ensure `source_id`, `target_id`, and `Calls` payload agree.

Expected invariant:

- `edge.source_id == calls.call_site_id`
- `edge.target_id == calls.callee_callable_id.or(calls.external_target_id)`
- unresolved calls use `target_id == None` and store `unresolved_target`
- `calls.caller_callable_id` is the owner/source callable for caller-oriented
  traversal

Done when:

- The call payload is self-explanatory as supergraph schema, not a copy of old
  `CallEdge`.

### SG-CLEAN-042: Normalize Index Names

Deliver:

- Review `GraphIndexes` fields:
  - `calls_by_caller`
  - `calls_by_callee`
  - `call_site_to_calls`
  - `caller_to_callee_calls`
  - `caller_to_call_targets`
- Keep them if they are useful supergraph indexes.
- Rename any field that encodes old call graph assumptions.

Done when:

- Index names describe supergraph traversal directly.
- Views use indexes instead of scanning full graph for common traversals.

## Phase 5: Documentation Cleanup

### SG-CLEAN-050: Update Architecture Docs

Deliver:

- Update `doc/supergraph.md` so it describes the final state directly.
- Remove or rewrite the section that says the existing standalone `CallGraph`
  can be regenerated from supergraph facts.
- Make the core decision explicit: call graph is a logical view only.

Done when:

- New readers see one architecture, not a migration story.

### SG-CLEAN-051: Retire `doc/ast-to-call-graph.md`

Deliver one of:

- Replace it with `doc/ast-to-supergraph.md`, or
- Rewrite it in place to describe AST-to-supergraph lowering, or
- Move any still-useful call resolution details into `doc/supergraph.md` and
  delete the old document.

Recommendation:

- Create or rename to `doc/ast-to-supergraph.md`.
- Keep language adapter details.
- Remove standalone call graph persistence language.

Done when:

- Documentation does not teach contributors to build a separate call graph.

### SG-CLEAN-052: Update Implementation Checklist

Deliver:

- Update `doc/impl/supergraph/supergraph-todo.md`.
- Replace `SG-001` wording about compatibility output with direct supergraph
  lowering.
- Add completed cleanup tasks or link to this plan if the checklist remains the
  implementation tracker.

Done when:

- The implementation checklist matches the code architecture.

### SG-CLEAN-053: Update Learnings And Index Docs

Deliver:

- Update `doc/impl/supergraph/supergraph-learnings.md` entries that mention
  compatibility preservation if they read as current design guidance.
- Update `doc/graph-index.md` references such as `CallGraphIndex`.

Done when:

- Docs consistently describe call graph behavior as supergraph views/indexes.

## Phase 6: Final Removal And Verification

### SG-CLEAN-060: Search For Historical Baggage

Deliver:

- Run searches and remove stale references:
  - `rg "CallGraph"`
  - `rg "call_graph"`
  - `rg "call-graph"`
  - `rg "compatibility"`
  - `rg "legacy"`
  - `rg "old"`

Expected remaining references:

- `CallGraphView` or equivalent logical view name may remain.
- Documentation may use "call graph" as a logical graph concept.
- There should be no `CallGraph` physical schema, no compatibility converter,
  and no call graph CLI command.

Done when:

- Remaining references are intentional and first-class.

### SG-CLEAN-061: Verify Determinism And Index Integrity

Deliver:

- Tests should assert:
  - nodes sorted by `node_id`
  - edges sorted by `edge_id`
  - IDs unique
  - `node_position_by_id` and `edge_position_by_id` match vectors
  - node and edge kind indexes match vector contents
  - caller/callee/call-site indexes match `Calls` edges
  - `source_span_to_nodes` remains valid
  - owner indexes remain valid

Done when:

- Direct construction produces the same deterministic guarantees as the old
  conversion path.

### SG-CLEAN-062: Run Full Verification

Deliver:

- Run:
  - `cargo fmt`
  - `cargo test`
  - targeted Python fixture tests
  - targeted TypeScript fixture tests
  - CLI tests
- Optionally compare before/after supergraph JSON for representative fixtures.

Done when:

- Full test suite passes.
- Any intentional JSON/schema changes are documented.

## Suggested Implementation Order

Implement in small PR-sized slices:

1. Add direct lowering module and adapter trait while leaving old code in place.
2. Switch `build_*_supergraph` to direct lowering.
3. Rewrite e2e tests to assert `ProgramSupergraph` and call views.
4. Remove `analyze_*_call_graph` APIs and CLI commands.
5. Delete `CallGraph` schema and conversion functions.
6. Rename remaining view/lowering symbols to first-class supergraph names.
7. Update docs and run final baggage search.

This order keeps behavior testable after each step while still requiring the
final state to be clean.

## Final Acceptance Criteria

The cleanup is complete only when all of the following are true:

- There is one physical graph type: `ProgramSupergraph`.
- There is one graph builder path: direct use of `ProgramSupergraphBuilder`.
- There is no `CallGraph` struct.
- There is no `src/call_graph` module.
- There are no whole-graph conversion functions.
- Public APIs return `ProjectAst` or `ProgramSupergraph`, not standalone logical
  graph structs.
- CLI commands emit AST JSON or supergraph JSON only.
- Tests assert supergraph facts and logical views directly.
- Documentation describes the supergraph as the current architecture, not a
  wrapper around call graph history.
- A reviewer can follow source parsing, lowering, enrichment, indexing, and view
  traversal without encountering duplicate graph implementations.
