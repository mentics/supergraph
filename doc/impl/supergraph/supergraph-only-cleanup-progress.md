# Supergraph-Only Cleanup Progress

This document tracks the subagent loop for
`doc/impl/supergraph/supergraph-only-cleanup-plan.md`.

## Loop State

- Status: complete/stopped
- Mode: dynamic, one cleanup slice per iteration
- Current focus: none; all SG-CLEAN items complete
- Next action: optional review/commit only if requested by the user
- Wake cadence: inactive
- Stop condition: met; all SG-CLEAN items are completed

## Item Status

| Item | Status | Notes |
| --- | --- | --- |
| SG-CLEAN-000 | complete | Baseline recorded in Iteration 1 evidence; full `cargo test` and targeted Python/TypeScript/CLI suites pass. |
| SG-CLEAN-001 | complete | Existing coverage mapped and one focused Python supergraph-native no-regression test added; broader rewrites belong to later cleanup phases. |
| SG-CLEAN-010 | complete | Added internal `analysis::source_graph` scaffold with `SourceGraphLowerer` and native `ProgramSupergraphBuilder` insertion API; public behavior remains on the existing path. |
| SG-CLEAN-011 | complete | Renamed the adapter role to `LanguageSupergraphAdapter`, moved adapter-facing call types to supergraph schema concepts, and kept call graph compatibility conversion inside the existing bridge. |
| SG-CLEAN-012 | complete | Internal `analysis::source_graph` now inserts base artifacts, scopes, callables, bindings, containment, binding, resolution, and unresolved-import diagnostic facts directly through `ProgramSupergraphBuilder`; public builders still use the old bridge until SG-CLEAN-015. |
| SG-CLEAN-013 | complete | Internal `analysis::source_graph` now resolves pending calls into call-site nodes, containment, external targets, `Calls` edges, and unresolved-call diagnostics through `ProgramSupergraphBuilder`; public builders still use the old bridge until SG-CLEAN-015. |
| SG-CLEAN-014 | complete | Internal direct lowerer now finalizes incoming local call counts and decorated callable metadata before `ProgramSupergraphBuilder::finish()`, without relying on the old `CallGraph` finish pass. |
| SG-CLEAN-015 | complete | Public Python and TypeScript supergraph builders now use the direct `analysis::source_graph` lowerer, then run semantic enrichment; focused and full verification pass. |
| SG-CLEAN-016 | complete | Added typed `ProgramSupergraphBuilder` helpers for the remaining semantic edge families and migrated builder fixture/raw semantic edge assembly where safe. |
| SG-CLEAN-020 | complete | Python and TypeScript e2e fixture/import-alias tests now use `analyze_*_supergraph`, `CallGraphView`, and direct `EdgeFact::Calls` assertions instead of standalone `CallGraph` fixture behavior. |
| SG-CLEAN-021 | complete | E2E compatibility equality/projection coverage is now direct supergraph shape plus call-view count/helper assertions; remaining internal `call_graph_from_supergraph` test usage is SG-CLEAN-023 scope. |
| SG-CLEAN-022 | complete | CLI tests now cover AST and supergraph commands without standalone call graph JSON expectations. |
| SG-CLEAN-023 | complete | Internal analysis module tests now use `ProgramSupergraph`, `CallGraphView`, and canonical supergraph call indexes instead of `call_graph_from_supergraph`. |
| SG-CLEAN-030 | complete | Removed public standalone CallGraph analysis APIs and crate-level `call_graph` exposure; old CLI call-graph commands now use a crate-internal bridge until SG-CLEAN-033. |
| SG-CLEAN-031 | complete | Deleted the private standalone `CallGraph` schema module; remaining call-related code uses supergraph schema types, with old CLI command names temporarily serializing `ProgramSupergraph` until SG-CLEAN-033. |
| SG-CLEAN-032 | complete | Confirmed whole-graph conversion functions and conversion-only helpers are absent; `src/supergraph/views.rs` contains view code only. |
| SG-CLEAN-033 | complete | Removed user-visible `call-graph-*` CLI commands, help text exposure, and the temporary crate-internal command bridge. |
| SG-CLEAN-034 | complete | Moved remaining adapter/language lowering code under `analysis::source_graph` and removed `src/call_graph.rs` plus the `src/call_graph/` directory. |
| SG-CLEAN-035 | complete | Removed `legacy_calls` fallback logic from `analysis::calls`; call emission now depends on call-site `ResolvesTo` facts and current unresolved-call invariants. |
| SG-CLEAN-040 | complete | Kept the call-oriented logical view and renamed transitional view/filter terms to first-class `CallGraphView`/`CallGraphFilter` names. |
| SG-CLEAN-041 | complete | `Calls` payloads now agree with call-site edge endpoints; unresolved/dynamic non-callable calls are targetless and retain target text in `unresolved_target`. |
| SG-CLEAN-042 | complete | Call indexes now distinguish all-call caller/call-site traversal from concrete-target traversal for callable/external targets. |
| SG-CLEAN-050 | complete | Updated `doc/supergraph.md` to describe direct `ProjectAst` -> `ProgramSupergraphBuilder` construction, semantic enrichment, and logical `CallGraphView` traversal without standalone `CallGraph` migration language. |
| SG-CLEAN-051 | complete | Added `doc/ast-to-supergraph.md` and deleted `doc/ast-to-call-graph.md`; contributor docs now describe direct `ProjectAst` -> `ProgramSupergraphBuilder` lowering and `CallGraphView` as a logical view. |
| SG-CLEAN-052 | complete | Updated `supergraph-todo.md` for current direct supergraph architecture, including `analysis::source_graph`, `ProgramSupergraphBuilder`, `CallGraphView`, targetless unresolved/dynamic calls, concrete-target indexes, and links to cleanup tracking. |
| SG-CLEAN-053 | complete | Updated broader learnings and graph index docs to describe call graph behavior as supergraph views/indexes. |
| SG-CLEAN-060 | complete | Historical baggage search classified remaining matches as first-class logical view references, cleanup-history tracking, negative CLI assertions, or unrelated compatibility/legacy wording; no additional stale live files needed removal. |
| SG-CLEAN-061 | complete | Verified direct ProgramSupergraph determinism and index integrity against vector contents, including current all-call and concrete-target call index semantics. |
| SG-CLEAN-062 | complete | Full verification passed and final acceptance searches found no live standalone `CallGraph` implementation. |

## Dispatch Log

### Iteration 1

- Focus: SG-CLEAN-000 / SG-CLEAN-001
- Subagent: launched
- Goal: inspect the current baseline, identify existing regression coverage, and make the smallest useful progress toward documented no-regression checks.
- Result: complete

#### SG-CLEAN-000 Evidence

- `cargo test --test python_incident_triage_e2e`: passed before edits, 7 tests.
- `cargo test --test typescript_incident_board_e2e`: passed before edits, 6 tests.
- `cargo test --test cli_supergraph`: passed before edits, 4 tests.
- `cargo test`: passed before edits, 130 lib tests, 0 main tests, 4 CLI integration tests, 7 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- No pre-existing test failures were observed.

#### SG-CLEAN-001 Coverage

- Existing coverage already checked deterministic supergraph JSON, sorted unique node/edge IDs, common supergraph indexes, compatibility fact counts against `CallGraph`, current call view counts/filters, unresolved calls with evidence, caller/callee compatibility indexes, and CLI `supergraph-python` / `supergraph-typescript` output.
- Existing `tests/supergraph_language_parity.rs` covers both languages through public supergraph views for structural facts, call targets, dynamic call uncertainty, SDG edges, requirements, traceability, sync classification, and invalidation.
- Added `preserves_incident_triage_python_supergraph_no_regression_facts_directly` in `tests/python_incident_triage_e2e.rs`. It asserts Python `ProgramSupergraph` facts directly for artifacts, module/class/function scopes, import/assignment/parameter/class/function/method bindings, matching `Binds` edges, module initializer callables, implicit constructors, local/external/unresolved `Calls` edges, external decorator targets, unresolved diagnostics, exact/external/unresolved resolution metadata, caller/callee/call-site indexes, incoming local call counts, and decorated callable external invocation metadata.
- Gaps intentionally left for later phases: TypeScript does not yet have an equivalent direct SG-CLEAN-001 checklist test, some existing e2e assertions still depend on `CallGraph` compatibility conversion, and CLI tests still cover old `call-graph-*` commands until SG-CLEAN-022 / SG-CLEAN-033.

#### Iteration 1 Verification

- `cargo fmt`: passed after edits.
- `cargo test --test python_incident_triage_e2e preserves_incident_triage_python_supergraph_no_regression_facts_directly`: passed after edits, 1 test.
- `cargo test --test python_incident_triage_e2e`: passed after edits, 8 tests.
- Linter check for `tests/python_incident_triage_e2e.rs`: no diagnostics.

### Iteration 2

- Focus: SG-CLEAN-010
- Subagent: launched
- Goal: create a compileable internal direct AST-to-supergraph lowering module scaffold without changing public behavior.
- Result: complete

#### SG-CLEAN-010 Result

- Added internal module `src/analysis/source_graph.rs` and declared it as `analysis::source_graph`.
- Introduced `SourceGraphLowerer`, `SourceGraphOptions`, and `build_initial_supergraph` as a compileable direct-lowering API.
- The scaffold lowers initial source facts into `ProgramSupergraphBuilder` rather than accumulating final standalone call graph-shaped vectors.
- Temporary state is named around source-lowering responsibilities: file contexts, local module/callable indexes, class summaries, short class names, and pending calls.
- Existing `analyze_*_supergraph` and `build_*_supergraph` entry points still use the prior implementation path; no public behavior was intentionally changed.

#### Iteration 2 Verification

- `cargo fmt`: passed after edits.
- `cargo check`: passed after edits.

#### Iteration 2 Learnings

- `ProgramSupergraphBuilder` already has the native node and edge insertion helpers needed for the first direct-lowering slice, including `add_artifact`, `add_scope`, `add_binding`, `add_callable`, `add_call_site`, `add_external_target`, `add_binds`, `add_contains`, `add_resolves_to_with_resolution`, and `add_calls`.
- The existing language-specific module path and call-resolution behavior still lives behind `CallGraphAdapter`; SG-CLEAN-011 should introduce the supergraph adapter trait before the new lowerer is wired into Python or TypeScript builders.
- The initial lowerer is intentionally unused until later slices switch construction paths, so the module allows dead code locally to keep verification output clean during the transition.

### Iteration 3

- Focus: SG-CLEAN-011
- Subagent: launched
- Goal: define the supergraph-era language adapter trait and move adapter naming/types toward supergraph schema without switching the construction path.
- Result: complete

#### SG-CLEAN-011 Result

- Replaced `CallGraphAdapter` with `LanguageSupergraphAdapter` in the adapter bridge while preserving existing public `build_*_call_graph` and `build_*_supergraph` behavior.
- Renamed resolved-call variants to supergraph-era concepts: `LocalTarget`, `ExternalTarget`, and `UnresolvedTarget`.
- Updated Python and TypeScript adapters to use supergraph schema types for adapter-facing callable kinds, dispatch kinds, call edge kinds, confidence, binding targets, and external target payloads.
- Localized compatibility conversions in `src/call_graph/core.rs`, where the existing bridge still constructs the temporary `CallGraph` until later direct-lowering slices remove it.
- Preserved external target confidence as bridge metadata because `supergraph::ExternalTarget` stores the payload while confidence belongs to graph facts; the old call graph compatibility path still serializes target confidence.

#### Iteration 3 Verification

- `cargo fmt`: passed after edits.
- `cargo check`: passed after edits.
- `cargo test --test python_incident_triage_e2e`: passed after edits, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed after edits, 6 tests.
- Linter check for edited adapter/bridge files: no diagnostics.

#### Iteration 3 Learnings

- The adapter responsibilities can move to supergraph vocabulary before switching construction paths, but `src/call_graph/core.rs` still needs compatibility conversions until SG-CLEAN-012 through SG-CLEAN-015 replace the temporary `CallGraph` builder state.
- `supergraph::ExternalTarget` intentionally omits a confidence field, so direct lowering should attach confidence to inserted external target nodes and call edges rather than the target payload.
- SG-CLEAN-012 can now focus on direct insertion of base nodes through `ProgramSupergraphBuilder` without also renaming adapter concepts.

### Iteration 4

- Focus: SG-CLEAN-012
- Subagent: launched
- Goal: move initial file/base fact lowering toward direct `ProgramSupergraphBuilder` insertion for artifacts, scopes, callables, bindings, `Contains`, `Binds`, `ResolvesTo`, and diagnostics.
- Result: complete

#### SG-CLEAN-012 Result

- Made the internal `SourceGraphLowerer` adapter-aware so it reuses `LanguageSupergraphAdapter` rules for language, analysis version, module paths, implicit constructor IDs, callable kinds, import normalization, imported-name targets, assignment target tracking, and field-flow context.
- Extended direct base insertion in `src/analysis/source_graph.rs` to cover source artifacts, module/class/function scopes, module initializer callables, implicit and explicit constructor callables, function/method callables, class/function/method/parameter/assignment/import bindings, `Contains`, `Binds`, `ResolvesTo`, and unresolved-import diagnostics via `ProgramSupergraphBuilder`.
- Kept call sites and final `Calls` edge resolution in pending source-lowering state for SG-CLEAN-013; no final graph-shaped vectors named `artifacts`, `scopes`, `bindings`, `callables`, `call_sites`, or `call_edges` were introduced.
- Preserved public behavior by leaving `build_python_supergraph` and `build_typescript_supergraph` on the existing bridge. `src/call_graph.rs` now exposes the adapter module as `pub(crate)` so the internal lowerer can reuse the SG-CLEAN-011 adapter trait during the transition.

#### Iteration 4 Verification

- `cargo fmt`: passed after edits.
- `cargo check`: passed after edits.
- `cargo test --test python_incident_triage_e2e`: passed after edits, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed after edits, 6 tests.
- Linter check for `src/analysis/source_graph.rs` and `src/call_graph.rs`: no diagnostics.

#### Iteration 4 Learnings

- Direct base lowering can share the existing adapter context types without reintroducing standalone graph-shaped output collections.
- Binding resolution edges can be emitted at binding insertion time for callable, class, module, and external targets; value and unresolved bindings correctly remain without `ResolvesTo` edges.
- Public behavior should not switch to the direct lowerer until SG-CLEAN-013 adds native call-site and `Calls` edge insertion and SG-CLEAN-014/015 settle incoming-call metadata and final construction.

### Iteration 5

- Focus: SG-CLEAN-013
- Subagent: launched
- Goal: extend the internal direct source lowerer to insert call-site, external-target, `Calls`, and unresolved-call diagnostic facts through `ProgramSupergraphBuilder`.
- Result: complete

#### SG-CLEAN-013 Result

- Added pending call resolution to the internal `SourceGraphLowerer` after import/assignment population and adapter field-flow enrichment, preserving the old resolution order before public builder paths are switched.
- Direct call lowering now inserts `CallSite` nodes, caller-callable-to-call-site `Contains` edges, deduplicated `ExternalTarget` nodes through the builder, `Calls` edges via `ProgramSupergraphBuilder::add_calls`, and unresolved-call diagnostics.
- `Calls` facts now use the call site as the edge source, local callable or external target as the edge target when available, caller/artifact ownership, call-site span, resolution-derived confidence, structured resolver evidence, and builder-derived uncertainty.
- Preserved the existing adapter resolution behavior for local direct calls, local method calls, constructors, decorators, imported functions/classes, module-object calls, external runtime/library calls, and dynamic/unresolved calls within the internal direct lowerer. Incoming local call counts and decorated callable metadata remain for SG-CLEAN-014.
- Public `build_*_supergraph` paths were intentionally left on the existing bridge; SG-CLEAN-015 owns switching public construction.

#### Iteration 5 Verification

- `cargo fmt`: passed after edits.
- `cargo check`: passed after edits.
- `cargo test --test python_incident_triage_e2e`: passed after edits, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed after edits, 6 tests.
- Linter check for `src/analysis/source_graph.rs`: no diagnostics.

#### Iteration 5 Learnings

- `ProgramSupergraphBuilder::add_calls` already encodes the desired calls edge invariant: `source_id` is the call site and `target_id` is the local callee or external target, so the direct lowerer only needs a small adapter-result helper around it.
- External target confidence belongs to the inserted node/edge facts, while `ExternalTarget` itself remains payload-only.
- Direct call resolution can reuse the SG-CLEAN-011 adapter behavior without a standalone call-edge collection; the remaining call metadata gap is the callable mutation currently performed by the old bridge finish pass.

### Iteration 6

- Focus: SG-CLEAN-014
- Subagent: launched
- Goal: preserve incoming local call counts and decorated callable external invocation metadata in the direct lowerer without relying on the old `CallGraph` finish pass.
- Result: complete

#### SG-CLEAN-014 Result

- Added a named `SourceGraphLowerer::finalize_callable_metadata` step that runs immediately before `ProgramSupergraphBuilder::finish()`.
- The direct lowerer now accumulates incoming local call counts as local `Calls` edges are emitted, then writes `Callable::incoming_local_call_count` into callable node payloads during finalization.
- Decorated callable metadata preserves the old bridge behavior by adding `decorated_callable` to callables with `decorator:` attributes, while avoiding duplicate metadata entries.
- Added a narrow `ProgramSupergraphBuilder::update_callables` helper so source lowering can mutate callable facts in-place without materializing a temporary `CallGraph`.
- Public `build_python_supergraph` and `build_typescript_supergraph` were intentionally left on the existing bridge; SG-CLEAN-015 owns switching them to the direct lowerer.

#### Iteration 6 Verification

- `cargo fmt`: passed.
- `cargo check`: passed.
- `cargo test analysis::source_graph::tests::finalizes_incoming_counts_and_decorated_metadata`: passed, 1 focused direct-lowerer test.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- Linter check for `src/analysis/source_graph.rs`, `src/supergraph/builder.rs`, and this progress document: no diagnostics.

#### Iteration 6 Learnings

- The old bridge computes incoming local call counts by counting call edges with a local `callee_callable_id`; the direct lowerer can preserve this while emitting calls rather than scanning a temporary `CallGraph`.
- `Callable::external_invocation_metadata` remains serialized supergraph payload, so decorated callable metadata should stay as a finalization mutation until a later schema/view decision removes or replaces the field.
- SG-CLEAN-015 can now switch public supergraph builders with the direct lowerer already covering base facts, calls, incoming local counts, and decorated callable metadata.

### Iteration 7

- Focus: SG-CLEAN-015
- Subagent: launched
- Goal: switch public Python and TypeScript supergraph builders to the direct lowerer while preserving semantic enrichment and deterministic builder/index output.
- Result: complete

#### SG-CLEAN-015 Result

- Switched `build_python_supergraph(project)` and `build_typescript_supergraph(project)` to build the initial graph through `analysis::source_graph::build_initial_supergraph`, then run `analysis::enrich_supergraph_with_semantic_flows` as the next stage.
- Kept `build_python_call_graph`, `build_typescript_call_graph`, `src/call_graph/core.rs`, old CLI commands, public call graph APIs, schemas, and conversion functions in place for later cleanup phases.
- Confirmed the direct lowerer finalizes through `ProgramSupergraphBuilder::finish()`, which refreshes uncertainty/provenance/fact identity, sorts nodes and edges, and rebuilds canonical indexes through `build_indexes`.
- Updated Python and TypeScript e2e supergraph tests to assert deterministic direct supergraph output and call-view behavior instead of exact compatibility equality with the standalone `CallGraph`.

#### Intentional Direct Output Difference

- Direct supergraph construction is no longer expected to round-trip exactly through `call_graph_from_supergraph(&supergraph) == analyze_*_call_graph(...)`. The direct path emits canonical supergraph facts, evidence/provenance, direct `Calls` edge IDs, and builder indexes first; the standalone `CallGraph` path remains available but is now a separate legacy construction path until later SG-CLEAN removal phases.
- The updated tests preserve the intended SG-CLEAN-015 guarantees instead: repeated supergraph output is stable, node/edge IDs are sorted and unique, canonical indexes are populated, and local/external/unresolved calls remain visible through `CallGraphView`.

#### Iteration 7 Verification

- `cargo fmt`: passed.
- `cargo check`: passed.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- `cargo test --test cli_supergraph`: passed, 4 tests.
- `cargo test`: passed, 131 lib tests, 0 main tests, 4 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- Linter checks for edited Rust files: no diagnostics.

#### Iteration 7 Learnings

- The public supergraph path can switch cleanly at the language builder boundary without removing or disturbing public call graph APIs.
- `ProgramSupergraphBuilder::finish()` already provides the deterministic ordering and canonical index rebuild required by SG-CLEAN-015.
- Compatibility equality tests are now migration baggage for public supergraph construction; later test cleanup phases should continue replacing `CallGraph` equality assertions with direct supergraph/view assertions.

### Iteration 8

- Focus: SG-CLEAN-016
- Subagent: launched
- Goal: add typed `ProgramSupergraphBuilder` helpers for remaining semantic edge families and migrate common normal analysis code away from raw edge assembly where safe.
- Result: complete

#### SG-CLEAN-016 Result

- Added typed `ProgramSupergraphBuilder` helpers for `ControlFlow`, `Controls`, `Defines`, `Uses`, `DataFlow`, `ParameterIn`, `ReturnsTo`, `ParameterOut`, `ThrowsTo`, `DecomposesTo`, `Conditions`, `Orders`, `TracesTo`, and `DependsOnDomainKnowledge`.
- Kept `ProgramSupergraphBuilder::insert_edge` as the low-level escape hatch, with the new semantic helpers sharing a private insertion path that uses stable family/source/target edge IDs, explicit owner/span, evidence, confidence, and finalization-refreshed uncertainty.
- Migrated builder tests and the all-family schema completeness fixture away from local raw `GraphEdge` assembly for these edge families where safe. Existing normal semantic enrichment still mutates an already-finished `ProgramSupergraph`, so broad graph-level refactoring was intentionally left out of this slice.

#### Iteration 8 Verification

- `cargo fmt`: passed.
- `cargo check`: passed.
- `cargo test`: passed, including 131 lib tests, 0 main tests, 4 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- Linter check for `src/supergraph/builder.rs` and this progress document: no diagnostics.

#### Iteration 8 Learnings

- `Defines` and `Uses` payloads do not always encode the graph edge target by themselves, so their typed helpers accept explicit source and target IDs.
- Builder helper coverage can be completed without removing the raw `insert_edge` escape hatch; tests now exercise the normal typed path for every SG-CLEAN-016 family.
- Semantic enrichment modules currently operate after builder finalization on `ProgramSupergraph`, so deeper migration there should be a separate graph-mutation API decision rather than a broad refactor inside SG-CLEAN-016.

### Iteration 9

- Focus: SG-CLEAN-020
- Subagent: launched
- Goal: rewrite Python and TypeScript fixture tests that still use standalone `CallGraph` assertions so they assert `ProgramSupergraph` facts or `CallGraphView` behavior directly.
- Result: complete

#### SG-CLEAN-020 Result

- Rewrote the Python and TypeScript fixture tests formerly named `analyzes_*_fixture_into_call_graph` to analyze with `analyze_*_supergraph`, create `CallGraphView`, and assert local/external call coverage through view-backed helpers.
- Converted generated Python and TypeScript import-alias call tests from `analyze_*_call_graph` to `analyze_*_supergraph` plus call view assertions.
- Replaced e2e helper shapes so local/external call assertions take `ProgramSupergraph` plus a call view, and callable/external lookups use supergraph node facts.
- Preserved unresolved call coverage with direct `EdgeFact::Calls` assertions for Python dynamic calls and the TypeScript `onAssignOwner` callback prop.
- Did not take on SG-CLEAN-021 compatibility equality tests; this slice only removed standalone `CallGraph` usage from the e2e fixture/import-alias tests.

#### Iteration 9 Verification

- Search for e2e standalone call graph fixture usages passed: no matches for `req_graph::call_graph::CallGraph`, `analyze_*_call_graph`, `assert_local_edge`, `assert_external_edge`, `callable_id(&CallGraph`, or `analyzes_*_fixture_into_call_graph` under `tests/`.
- `cargo fmt`: passed.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo check`: passed.
- Linter check for edited e2e files: no diagnostics before final formatting.

#### Iteration 9 Learnings

- Supergraph artifact facts do not carry per-artifact language; language is asserted at the `ProgramSupergraph` level while artifact presence is checked through `NodeFact::Artifact`.
- `CallGraphView::calls_from_caller_to_target` is sufficient for fixture local/external call behavior checks once qualified names are resolved through supergraph callable and external target nodes.
- The e2e import-alias tests were small enough to convert in SG-CLEAN-020 because they used the same legacy `CallGraph` helpers and covered call behavior rather than compatibility equality.

### Iteration 10

- Focus: SG-CLEAN-021
- Subagent: launched
- Goal: replace remaining compatibility equality/projection assertions with direct supergraph shape and call-view assertions.
- Result: complete

#### SG-CLEAN-021 Result

- Confirmed the old e2e whole-graph equality helpers `assert_equivalent_fact_counts` and `assert_equivalent_call_view` are no longer present.
- Strengthened Python and TypeScript direct supergraph e2e tests with call-view shape assertions that compare `CallGraphView::call_edges(CallGraphFilter::all_calls())` to canonical `EdgeFact::Calls` edges.
- Added direct call-view coverage for all call edge count, local call view count, local/external/unresolved call classes, `calls_from_caller_to_target`, and `calls_from_call_site` helper behavior.
- Kept public conversion APIs and compatibility functions in place, per later cleanup phases. Search still finds `call_graph_from_supergraph` in `src/analysis/symbols.rs` internal tests; SG-CLEAN-023 explicitly owns those internal module rewrites.

#### Iteration 10 Verification

- Search for `assert_equivalent_fact_counts`, `assert_equivalent_call_view`, and e2e `call_graph_from_supergraph(&...)` compatibility assertions: no e2e matches.
- `cargo fmt`: passed.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- Linter check for edited e2e files: no diagnostics.

#### Iteration 10 Learnings

- The remaining e2e regression value is now better expressed as direct supergraph invariants plus indexed `CallGraphView` behavior, not as projection compatibility.
- `CallGraphFilter::local_calls()` is useful for checking the view's non-external/non-unresolved local slice, while `CallGraphFilter::all_calls()` plus direct `EdgeFact::Calls` classification covers local callable, external target, and unresolved call families.
- The last `call_graph_from_supergraph` test references are internal symbol-analysis tests, so they should be handled with SG-CLEAN-023 rather than mixed into SG-CLEAN-021.

### Iteration 11

- Focus: SG-CLEAN-022
- Subagent: launched
- Goal: remove `call-graph-*` command coverage from CLI tests while preserving `analyze-*`, `supergraph-*`, unsupported-file, and deterministic supergraph JSON coverage.
- Result: complete

#### SG-CLEAN-022 Result

- Removed `call-graph-python` command coverage from `tests/cli_supergraph.rs`; no `call-graph-typescript` coverage was present in the file.
- Replaced the old mixed analysis/call-graph CLI test with AST JSON coverage for both `analyze-python` and `analyze-typescript`.
- Preserved deterministic JSON coverage for both `supergraph-python` and `supergraph-typescript`.
- Expanded unsupported file handling coverage to check both `supergraph-python` rejecting a `.ts` file and `supergraph-typescript` rejecting a `.py` file.
- Left CLI command implementations untouched; SG-CLEAN-033 owns removing `call-graph-*` commands from the public CLI.

#### Iteration 11 Verification

- Search in `tests/cli_supergraph.rs` for `call-graph-python`, `call-graph-typescript`, standalone call graph JSON expectations, and call graph naming: no matches after edits.
- `cargo fmt`: passed.
- `cargo test --test cli_supergraph`: passed, 4 tests.

#### Iteration 11 Learnings

- Existing CLI supergraph tests already covered deterministic `ProgramSupergraph` JSON for both languages; SG-CLEAN-022 only needed to remove old standalone command expectations and fill the missing `analyze-typescript` / TypeScript unsupported-file coverage.
- SG-CLEAN-023 should now remove internal module test usage of `call_graph_from_supergraph`, especially the references already noted in `src/analysis/symbols.rs`.

### Iteration 12

- Focus: SG-CLEAN-023
- Subagent: launched
- Goal: replace internal analysis test compatibility projections with `ProgramSupergraph`, `CallGraphView`, direct graph queries, or indexes while preserving symbol resolution and call traversal coverage.
- Result: complete

#### SG-CLEAN-023 Result

- Updated `src/analysis/symbols.rs` internal tests to remove the final `call_graph_from_supergraph` import and projection assertions.
- Replaced the TypeScript local self-call compatibility check with direct `CallGraphView`, `calls_by_caller`, and target index assertions over the canonical `Calls` edge.
- Replaced the compatibility projection count check with a direct `call_site_to_calls` index coverage assertion that ensures canonical `Calls` edges are indexed without extra summary duplicates.
- Preserved existing symbol resolution coverage for Python/TypeScript lexical symbols, uses, call-site `ResolvesTo` edges, dynamic-call possible bindings, unresolved calls, and caller-to-target traversal.

#### Iteration 12 Verification

- Search under `src/analysis` for `call_graph_from_supergraph`: no matches.
- Repository-wide Rust search still finds `call_graph_from_supergraph` only in the public compatibility bridge/implementation, which is later SG-CLEAN scope.
- `cargo fmt`: passed.
- `cargo test analysis::symbols::tests`: passed, 7 focused internal module tests.
- Linter check for `src/analysis/symbols.rs`: no diagnostics.

#### Iteration 12 Learnings

- The remaining internal compatibility projections were redundant with existing supergraph-native coverage; the tests already had direct call-site, resolution, and view assertions around the same behavior.
- `calls_by_caller`, concrete-target call indexes, and `call_site_to_calls` are sufficient to preserve call traversal/index coverage in internal analysis tests without materializing a standalone `CallGraph`.
- SG-CLEAN-030 can now remove public call graph APIs without being blocked by internal analysis module test imports of `call_graph_from_supergraph`.

### Iteration 13

- Focus: SG-CLEAN-030
- Subagent: launched
- Goal: remove public standalone `CallGraph` analysis APIs and re-exports while leaving schema, conversions, CLI commands, and module deletion to later SG-CLEAN items.
- Result: complete

#### SG-CLEAN-030 Result

- Removed `analyze_python_call_graph` and `analyze_typescript_call_graph` from `src/analysis.rs`.
- Removed the corresponding public crate-root re-exports from `src/lib.rs`.
- Changed `src/lib.rs` from `pub mod call_graph` to private `mod call_graph`, so the standalone physical graph module is no longer part of the public crate API.
- Narrowed `build_python_call_graph` and `build_typescript_call_graph` to `pub(crate)` and re-exported them only crate-internally from `src/call_graph.rs`.
- Preserved the existing `call-graph-python` and `call-graph-typescript` CLI commands for SG-CLEAN-033 by having `src/cli.rs` parse with public AST APIs, then call the crate-internal call graph builders as a temporary bridge.
- Left `src/call_graph/schema.rs`, conversion functions, CLI command deletion, and directory/module deletion untouched for SG-CLEAN-031 through SG-CLEAN-034.

#### Iteration 13 Verification

- Search under `src` for public removed API names passed: no `pub mod call_graph`, no public `analyze_*_call_graph`, and no public `build_*_call_graph` exports remain.
- Repository search still finds `build_*_call_graph` only in the crate-internal CLI bridge and crate-internal `src/call_graph` implementation; this is intentional until SG-CLEAN-033 removes the old commands.
- `cargo fmt && cargo check`: passed.
- `cargo test`: passed, including 131 lib tests, 0 main tests, 4 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- Linter check for edited Rust files: no diagnostics.

#### Iteration 13 Learnings

- The public analysis surface can now be limited to AST and supergraph entry points without deleting the old physical schema yet.
- Keeping the CLI command compatibility bridge crate-internal avoids reintroducing removed public Rust APIs while leaving user-visible command removal to SG-CLEAN-033.
- Making `call_graph` private at the crate root still allows SG-CLEAN-031 and SG-CLEAN-032 to remove schema and conversion internals in separate, focused slices.

### Iteration 14

- Focus: SG-CLEAN-031
- Subagent: launched
- Goal: remove the private standalone `CallGraph` schema and update remaining call-related code to use supergraph schema types.
- Result: complete

#### SG-CLEAN-031 Result

- Deleted `src/call_graph/schema.rs` and removed the `mod schema` declaration/export from `src/call_graph.rs`.
- Replaced the old schema-backed call graph builder core with a narrow temporary bridge that builds and serializes the canonical `ProgramSupergraph` for the legacy `call-graph-*` CLI commands until SG-CLEAN-033 removes those commands.
- Updated Python and TypeScript call graph bridge entry points to return `ProgramSupergraph` instead of the removed `CallGraph` type.
- Removed the old `call_graph_to_supergraph` / `call_graph_from_supergraph` conversion implementation from `src/supergraph/views.rs` because it could not compile without the removed schema; SG-CLEAN-032 should now verify and remove any remaining conversion-function documentation or leftovers.
- Confirmed no source code refers to `req_graph::call_graph::schema`, `crate::call_graph::schema`, or `super::schema` under `src/call_graph`.

#### Iteration 14 Verification

- `cargo fmt && cargo check`: passed.
- `cargo test`: passed, including 131 lib tests, 0 main tests, 4 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- Direct file check confirmed `src/call_graph/schema.rs` is absent.

#### Iteration 14 Learnings

- The remaining old `call-graph-*` CLI commands no longer justify preserving a private graph schema; a small documented bridge can serialize `ProgramSupergraph` until the command names are removed.
- Once the schema module is gone, whole-graph conversion functions become compile-time dead weight rather than useful compatibility, so SG-CLEAN-032 should be mostly verification and stale-reference cleanup.

### Iteration 15

- Focus: SG-CLEAN-032
- Subagent: launched
- Goal: remove remaining whole-graph conversion functions, conversion-only helpers, and stale conversion references so `src/supergraph/views.rs` contains views only.
- Result: complete

#### SG-CLEAN-032 Result

- Repository search found no Rust code references to `call_graph_to_supergraph` or `call_graph_from_supergraph`.
- Confirmed `src/supergraph/views.rs` contains `CallGraphView` and other read-only views/index-backed helpers, with no whole-graph compatibility conversion functions.
- Confirmed conversion-only schema/helper leftovers are gone with `src/call_graph/schema.rs` deleted and no active `mod schema` use from `src/call_graph`.
- Left `call-graph-*` CLI commands and their narrow crate-internal `ProgramSupergraph` bridge untouched for SG-CLEAN-033, as required.

#### Iteration 15 Verification

- `rg "call_graph_to_supergraph|call_graph_from_supergraph"`: no Rust code matches; remaining matches are historical cleanup-plan/progress/todo documentation.
- `rg "mod schema|schema::|CallGraph|CallNode|CallEdge|CallGraphIndex|call_graph_to_supergraph|call_graph_from_supergraph" src`: no standalone schema or conversion function references remain; remaining `CallGraphView` names are logical view terms and remaining CLI command names are SG-CLEAN-033 scope.
- Direct file check: `src/call_graph/schema.rs` is absent in the working tree.
- `cargo fmt`: passed.
- `cargo check`: passed.

#### Iteration 15 Learnings

- SG-CLEAN-031 already removed the conversion implementation as part of deleting the standalone schema; SG-CLEAN-032 mainly needed to verify there were no surviving exports, tests, docs-in-code, or conversion-only helpers.
- `src/supergraph/views.rs` now reads as a collection of indexed logical views; call graph naming there refers to a view over `ProgramSupergraph`, not a physical graph conversion.
- The next cleanup can remove the temporary `call-graph-*` CLI commands without needing any whole-graph conversion compatibility layer.

### Iteration 16

- Focus: SG-CLEAN-033
- Subagent: launched
- Goal: remove user-visible `call-graph-*` CLI commands, help text, and the temporary crate-internal command bridge.
- Result: complete

#### SG-CLEAN-033 Result

- Removed `Command::CallGraphPython` and `Command::CallGraphTypescript` from the clap subcommand enum.
- Removed the `run_with_args` branches that parsed AST input and serialized the temporary standalone-call-graph command output.
- Removed the now-unused `src/cli.rs` bridge imports for `build_python_call_graph` and `build_typescript_call_graph`.
- Removed the temporary crate-internal `build_python_call_graph`, `build_typescript_call_graph`, and `build_call_graph` bridge functions and re-exports while leaving the broader `src/call_graph` module in place for SG-CLEAN-034.
- Added CLI help coverage proving `req-graph --help` lists only `analyze-python`, `analyze-typescript`, `supergraph-python`, and `supergraph-typescript`, and does not expose old `call-graph-*` command names.

#### Iteration 16 Verification

- `rg "call-graph-python|call-graph-typescript" src`: no matches.
- `rg "call-graph-python|call-graph-typescript" tests`: matches only the new negative help assertions.
- `rg "CallGraphPython|CallGraphTypescript|build_python_call_graph|build_typescript_call_graph|build_call_graph"`: no code matches; remaining matches are historical cleanup-plan/progress references.
- Linter check for edited Rust files: no diagnostics.
- `cargo fmt && cargo check && cargo test --test cli_supergraph && cargo test`: passed, including 5 CLI integration tests and the full suite.

#### Iteration 16 Learnings

- Once the old command names are removed, the SG-CLEAN-031 temporary bridge can disappear cleanly; only the shared `id` helper remains in `src/call_graph/core.rs` until SG-CLEAN-034 moves or deletes the module.
- A small integration test around `--help` is enough to keep the user-visible CLI surface centered on AST and supergraph commands while later slices delete internal module leftovers.

### Iteration 17

- Focus: SG-CLEAN-034
- Subagent: launched
- Goal: move or delete remaining `src/call_graph` module contents, delete the module files/directories, and leave only intentional logical view references in source/tests.
- Result: complete

#### SG-CLEAN-034 Result

- Moved the remaining supergraph adapter trait/context types from `src/call_graph/adapter.rs` into `src/analysis/source_graph/adapter.rs`.
- Moved Python and TypeScript supergraph adapter/lowering entry points into `src/analysis/source_graph/python.rs` and `src/analysis/source_graph/typescript.rs`.
- Updated `analysis::source_graph` to own and expose crate-internal `build_python_supergraph` / `build_typescript_supergraph` wrappers.
- Updated analysis entry points and internal tests to import the builders from `analysis::source_graph` instead of `crate::call_graph`.
- Removed the private crate module declaration, deleted `src/call_graph.rs`, deleted the remaining files under `src/call_graph/`, and removed the empty directory.

#### Iteration 17 Verification

- `rg "call_graph" src tests`: only logical view references remain: `ProgramSupergraph::call_graph_view()` and test calls to that view.
- `cargo fmt && cargo check && cargo test`: passed after edits; full suite passed.
- Linter check for edited Rust files: no diagnostics.

#### Iteration 17 Learnings

- The surviving `src/call_graph` code was already supergraph adapter/language-lowering behavior, so it moved cleanly under `analysis::source_graph` without preserving call graph module naming.
- `src/call_graph/core.rs` only duplicated `supergraph::stable_id`; the moved language adapters now use the canonical helper directly.
- SG-CLEAN-035 remains necessary: `src/analysis/calls.rs` still has `legacy_calls`, `legacy_by_site*`, and `legacy_call` preservation logic that snapshots existing `Calls` edges.

### Iteration 18

- Focus: SG-CLEAN-035
- Subagent: launched
- Goal: remove legacy `Calls` edge preservation from `analysis::calls::emit` so call emission depends on canonical call-site, `ResolvesTo`, and unresolved-call facts.
- Result: complete

#### SG-CLEAN-035 Result

- Removed the `legacy_calls`, `legacy_by_site*`, `legacy_call`, and old payload preservation path from `src/analysis/calls.rs`.
- `analysis::calls::emit` now drops existing `Calls` edges and rebuilds them only from `CallSite` nodes plus call-site `ResolvesTo` edges to callable, external-target, or binding nodes.
- Preserved unresolved call behavior as a current supergraph invariant: unresolved call-site resolutions target placeholder external-target nodes, keep `unresolved_target`, and lower to `CallEdgeKind::PossibleDynamic` without reading older `Calls` payloads.
- Added focused `analysis::calls` unit tests covering exact local, external, possible dynamic, unresolved, and stale pre-existing call edge cases.

#### Iteration 18 Verification

- `rg "legacy|compatib" src/analysis/calls.rs -i`: no matches.
- `cargo fmt`: passed.
- `cargo test analysis::calls::tests`: passed, 2 tests.
- `cargo check`: passed.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- Linter check for `src/analysis/calls.rs` and this progress document: no diagnostics.

#### Iteration 18 Learnings

- `symbols::emit` is the bridge that turns direct-lowerer `Calls` facts into canonical call-site `ResolvesTo` edges; `calls::emit` can then rebuild `Calls` without carrying forward older call payloads.
- Unresolved calls currently survive by resolving call sites to placeholder `ExternalTarget` nodes while keeping `Calls::unresolved_target`; this is a supergraph-native invariant, not a fallback to a targetless old edge.
- The removed preservation branch had been masking unresolved direct calls as `PossibleDynamic`; that behavior is now encoded explicitly from `Resolution::Unresolved`.

### Iteration 19

- Focus: SG-CLEAN-040
- Subagent: launched
- Goal: keep the call-oriented logical view but rename `Current*` view/filter terms to first-class supergraph view names.
- Result: complete

#### SG-CLEAN-040 Result

- Kept the call-oriented logical view because `call_graph_view()` still reads as a useful graph view over `ProgramSupergraph`.
- Renamed the transitional call view/filter types to `CallGraphView` and `CallGraphFilter` across `src/supergraph/views.rs`, analysis tests, e2e tests, language parity tests, and cleanup docs.
- Kept `ProgramSupergraph::call_graph_view()` and updated its return type to `CallGraphView<'_>`, matching the existing `control_flow_view`, `data_flow_view`, and dependence graph view accessors.
- Removed concrete transitional and compatibility-style call view names from source, tests, and docs; the cleanup plan now describes discouraged prefixes instead of carrying old example type names.

#### Iteration 19 Verification

- `cargo fmt`: passed.
- Exact transitional and compatibility-style call view name search: no matches.
- `cargo check`: passed.
- `cargo test analysis::symbols::tests`: passed, 7 focused internal analysis tests.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- `cargo test`: passed, including 133 lib tests, 0 main tests, 5 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.

#### Iteration 19 Learnings

- The surrounding view API consistently uses graph-view names such as `ControlFlowGraphView`, `DataFlowGraphView`, and `ProgramDependenceGraphView`, so `CallGraphView` is the best final first-class name.
- The `call_graph_view()` method remains clear as a logical graph view accessor over `ProgramSupergraph`; renaming it would add churn without improving readability.
- SG-CLEAN-041 can now review call payload shape without also carrying transitional view/filter names.

### Iteration 20

- Focus: SG-CLEAN-041
- Subagent: launched
- Goal: normalize `EdgeFact::Calls` payloads and edge endpoints so call facts read as current supergraph schema rather than old call edge compatibility records.
- Result: complete

#### SG-CLEAN-041 Result

- Kept `Calls::caller_callable_id` as the caller-oriented traversal owner and kept only `callee_callable_id` / `external_target_id` as concrete denormalized call targets.
- Updated `analysis::calls::emit` so rebuilt `Calls` edges use `source_id == calls.call_site_id` and `target_id == calls.callee_callable_id.or(calls.external_target_id)`.
- Aligned unresolved calls with the expected invariant: unresolved placeholder `ResolvesTo` facts may still exist as resolution evidence, but emitted `Calls` edges are targetless and store the unresolved expression in `unresolved_target`.
- Also made possible dynamic binding calls targetless at the `Calls` layer because a binding is not a concrete callable or external call target; call-site resolution evidence remains available through `ResolvesTo`.
- Removed the call-view filtering fallback that treated payload fields as alternate edge endpoints, while preserving caller and call-site traversal through existing indexes.
- Added and updated analysis/e2e tests to assert the endpoint/payload invariant directly.

#### Iteration 20 Verification

- `cargo fmt`: passed.
- `cargo check`: passed.
- `cargo test analysis::symbols::tests`: passed, 7 tests.
- `cargo test analysis::calls::tests`: passed, 2 tests.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- `cargo test`: passed, including 133 lib tests, 0 main tests, 5 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- Linter check for edited Rust files: no diagnostics.

#### Iteration 20 Learnings

- `ResolvesTo` can preserve resolution evidence for placeholder external targets or dynamic bindings without forcing `Calls` edges to target those placeholder/non-callable nodes.
- `Calls` target indexes already tolerate targetless edges: caller and call-site indexes retain traversal, while callee/target indexes remain limited to concrete callable or external targets.
- SG-CLEAN-042 can now review index names with a cleaner distinction between caller/call-site traversal and concrete call-target traversal.

### Iteration 21

- Focus: SG-CLEAN-042
- Subagent: launched
- Goal: review and normalize call-related `GraphIndexes` names so they describe current supergraph traversal and concrete call-target behavior directly.
- Result: complete

#### SG-CLEAN-042 Result

- Kept `calls_by_caller` and `call_site_to_calls` because they index all `Calls` edges, including targetless unresolved and dynamic non-callable calls.
- Renamed target-specific indexes to `calls_by_concrete_target`, `caller_to_concrete_target_calls`, and `caller_to_concrete_call_targets` so the schema makes clear that these cover only concrete callable or external target nodes.
- Updated `CallGraphView` to use the renamed concrete-target indexes for common caller-to-target and target-filtered traversals.
- Strengthened Python e2e index assertions so targetless calls are required to remain in caller/call-site indexes and absent from concrete-target indexes.

#### Iteration 21 Verification

- `cargo fmt`: passed.
- Linter check for edited Rust/test files: no diagnostics.
- `cargo check`: passed.
- `cargo test analysis::symbols::tests`: passed, 7 tests.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- `cargo test`: passed, including 133 lib tests, 0 main tests, 5 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.

#### Iteration 21 Learnings

- Caller and call-site call indexes are all-call traversal indexes and should retain targetless unresolved/dynamic calls.
- Concrete target indexes are intentionally narrower: they include only `Calls` edges whose edge target is a callable or external target, matching the SG-CLEAN-041 endpoint invariant.
- `CallGraphView::calls_from_caller_to_target` and `call_targets_from_caller` should continue to answer concrete target traversal from indexes rather than scanning the full graph.

### Iteration 22

- Focus: SG-CLEAN-050
- Subagent: launched
- Goal: update `doc/supergraph.md` so it presents direct `ProgramSupergraph` architecture and call graph as a logical view only.
- Result: complete

#### SG-CLEAN-050 Result

- Rewrote `doc/supergraph.md` to present the final architecture directly: source discovery, `ProjectAst` parsing, direct source-graph lowering through `ProgramSupergraphBuilder`, semantic enrichment, and logical views over `ProgramSupergraph`.
- Removed migration-era wording that suggested a standalone `CallGraph` schema could become compatibility output or be regenerated from supergraph facts.
- Made the core call graph decision explicit: call graph behavior is provided by `CallGraphView` over `ProgramSupergraph`, with `CallGraphFilter`, targetless unresolved/dynamic calls, all-call caller/call-site indexes, and concrete-target call indexes.
- Left `doc/ast-to-call-graph.md` untouched for SG-CLEAN-051.

#### Iteration 22 Verification

- Focused stale-wording search in `doc/supergraph.md`: `rg "CallGraph|call graph|call_graph|call-graph|regenerated|compatibility|migration|standalone|existing" doc/supergraph.md`.
- Remaining matches are intentional logical-view references, an explicit statement that no standalone physical `CallGraph` model exists, the JSON compatibility document reference, and generic existing semantic-edge wording.
- No Rust tests were run because this slice changed documentation only.

#### Iteration 22 Learnings

- Architecture docs should introduce `CallGraphView` as the first-class call surface instead of narrating how an older standalone graph was removed.
- The current call terminology to preserve in docs is `CallGraphView`, `CallGraphFilter`, targetless unresolved/dynamic calls, all-call caller/call-site indexes, and concrete-target call indexes.
- SG-CLEAN-051 should now retire or rewrite `doc/ast-to-call-graph.md` so contributor onboarding no longer points at a standalone call graph lowering document.

### Iteration 23

- Focus: SG-CLEAN-051
- Subagent: launched
- Goal: replace or rewrite `doc/ast-to-call-graph.md` as AST-to-supergraph lowering documentation while preserving useful adapter and call resolution details.
- Result: complete

#### SG-CLEAN-051 Result

- Added `doc/ast-to-supergraph.md` as the contributor guide for direct AST-to-supergraph lowering.
- Deleted `doc/ast-to-call-graph.md` so documentation no longer teaches contributors to construct or persist a separate physical call graph.
- Preserved useful language adapter, symbol resolution, call resolution, external target, entrypoint, boundary evidence, incremental update, diagnostics, and test strategy guidance while reframing it around `ProjectAst`, `ProgramSupergraphBuilder`, semantic enrichment, and logical `CallGraphView` traversal.
- Removed standalone call graph persistence/schema language, including the old per-file call graph JSON guidance.

#### Iteration 23 Verification

- `rg "standalone call graph|separate call graph|persist(ed)? call graph|call_edges\\.json|callables\\.json|build a call graph|call graph construction|call graph schema|CallGraph schema|call graph conversion|CallGraph conversion|call_graph_to_supergraph|call_graph_from_supergraph" doc/ast-to-supergraph.md -i`: no matches.
- `rg "ast-to-call-graph\\.md" doc`: remaining matches are cleanup-plan/progress tracking mentions of the retired document; `ReadFile doc/ast-to-call-graph.md` confirmed the file is absent.
- No Rust tests were run because this slice changed documentation only.

#### Iteration 23 Learnings

- The old AST-to-call-graph guide mostly contained still-useful adapter and resolver details; the key cleanup was changing the target architecture from standalone graph records to direct typed supergraph facts.
- Contributor-facing call documentation should describe `Calls` edges as call-site-sourced supergraph edges and reserve `CallGraphView` for traversal, filtering, and aggregation over those facts.
- SG-CLEAN-052 should now update the implementation checklist without reworking the newly retired AST-to-supergraph guide.

### Iteration 24

- Focus: SG-CLEAN-052
- Subagent: launched
- Goal: update `doc/impl/supergraph/supergraph-todo.md` so the implementation checklist matches direct `ProgramSupergraph` architecture and links to cleanup tracking.
- Result: complete

#### SG-CLEAN-052 Result

- Added checklist context linking the implementation tracker to `doc/impl/supergraph/supergraph-only-cleanup-plan.md` and this progress tracker for the completed supergraph-only cleanup history.
- Rewrote `SG-001` around direct `ProjectAst` lowering through `analysis::source_graph` into `ProgramSupergraphBuilder`, followed by semantic enrichment over the canonical `ProgramSupergraph`.
- Updated `SG-001` completion criteria to match the current architecture: `CallGraphView` is the call-oriented traversal surface, with no `src/call_graph`, standalone physical `CallGraph`, whole-graph conversion path, or `call-graph-*` CLI commands.
- Updated call-view checklist wording so unresolved and dynamic non-callable calls are targetless `Calls` edges, caller/call-site indexes cover all calls, and concrete-target indexes cover callable/external-target traversal.
- Renamed the JSON checklist task from compatibility/migration wording to schema versioning/evolution wording.

#### Iteration 24 Verification

- `rg "compatibility|CallGraph|call_graph|call-graph|standalone|src/call_graph|call graph" doc/impl/supergraph/supergraph-todo.md -i`: before edits, stale matches appeared in `SG-001`, call view acceptance text, and JSON schema wording.
- Focused post-edit searches in `doc/impl/supergraph/supergraph-todo.md` confirmed remaining matches are intentional: logical `CallGraphView` / call graph view references, the cleanup-history link context, and explicit absence criteria for `src/call_graph`, standalone physical `CallGraph`, whole-graph conversion paths, and `call-graph-*` CLI commands.
- No Rust tests were run because this slice changed documentation only.

#### Iteration 24 Learnings

- `supergraph-todo.md` can remain the implementation checklist if it reads as current-state architecture and delegates cleanup history to the dedicated cleanup plan/progress docs.
- `SG-001` is the right checklist anchor for the direct architecture: `analysis::source_graph` plus `ProgramSupergraphBuilder`, not compatibility output from another graph model.
- SG-CLEAN-053 should now update broader learnings and index docs, especially any remaining compatibility-preservation guidance or old call index names.

### Iteration 25

- Focus: SG-CLEAN-053
- Subagent: launched
- Goal: update broader learnings and index docs so current guidance describes call graph behavior as supergraph views and indexes.
- Result: complete

#### SG-CLEAN-053 Result

- Updated `doc/impl/supergraph/supergraph-learnings.md` entries that read like current call graph compatibility guidance, replacing standalone projection, placeholder-target, and old caller-to-callee summary wording with current `ProgramSupergraph`, `Calls`, `ResolvesTo`, and `CallGraphView` conventions.
- Updated `doc/graph-index.md` so runtime traversal guidance starts from logical supergraph views, treats `ProgramSupergraph` as canonical, and names current all-call and concrete-target call indexes instead of `CallGraphIndex`.
- Preserved intentional historical cleanup references in the cleanup plan/progress docs and intentional logical view wording such as `CallGraphView`.

#### Iteration 25 Verification

- `rg "compatibility|CallGraphIndex|call_graph|call-graph|standalone|conversion|converted|migration|caller_to_callee|caller_to_call_targets|callee|placeholder target|placeholder call target" doc/impl/supergraph/supergraph-learnings.md -i`: remaining matches are non-call-graph compatibility notes, value/interprocedural callee terminology, SG-031 placeholder resolution targets, or historical learning context; no stale standalone `CallGraph` or old call index guidance remains.
- `rg "compatibility|CallGraphIndex|call_graph|call-graph|standalone|conversion|converted|migration|caller_to_callee|caller_to_call_targets|callee|placeholder target|placeholder call target" doc/graph-index.md -i`: no matches.
- Linter check for `doc/impl/supergraph/supergraph-learnings.md` and `doc/graph-index.md`: no diagnostics.
- No Rust tests were run because this slice changed documentation only.

#### Iteration 25 Learnings

- Broader learnings can keep historical phase order while still avoiding current guidance that preserves removed standalone call graph compatibility paths.
- Index docs should name the concrete `GraphIndexes` split directly: `calls_by_caller` and `call_site_to_calls` are all-call traversal indexes, while `calls_by_concrete_target`, `caller_to_concrete_target_calls`, and `caller_to_concrete_call_targets` are concrete-target traversal indexes.
- SG-CLEAN-060 should now run the repository-wide stale-reference pass while preserving intentional cleanup-history references and first-class logical view names.

### Iteration 26

- Focus: SG-CLEAN-060
- Subagent: launched
- Goal: search for historical standalone call graph baggage and remove or classify stale references.
- Result: complete

#### SG-CLEAN-060 Result

- Ran the requested historical-baggage searches for `CallGraph`, `call_graph`, `call-graph`, `compatibility`, `legacy`, and `old`.
- Classified remaining source/test matches as intentional first-class references: `CallGraphView`, `CallGraphFilter`, `ProgramSupergraph::call_graph_view()`, tests that exercise those logical view APIs, and CLI help tests that assert removed `call-graph-*` commands are absent.
- Classified remaining documentation matches in `doc/impl/supergraph/supergraph-only-cleanup-plan.md` and this progress tracker as historical cleanup tracking, not current architecture guidance.
- Classified `doc/supergraph-json-compatibility.md`, `doc/versioned-graph.md`, non-call-graph `legacy` test strings, placeholder wording, and `threshold` fixture text as unrelated to removed standalone call graph baggage.
- Confirmed the stale physical files and retired AST-to-call-graph guide are already deleted in the working tree: `src/call_graph.rs`, `src/call_graph/`, and `doc/ast-to-call-graph.md` are present only as deleted paths in git status or historical tracker references.
- No additional stale live references required removal.

#### Iteration 26 Verification

- `rg --no-messages --glob '!target/**' "CallGraph|call_graph|call-graph|compatibility|legacy|old" src tests doc`: completed; remaining matches were classified as intentional first-class references, historical tracking references, negative CLI assertions, or unrelated terminology.
- `ls -la "src" "src/call_graph" "doc"`: confirmed `src/call_graph` and `doc/ast-to-call-graph.md` are absent from the live filesystem.
- `git status --short -- "src/call_graph.rs" "src/call_graph" "doc/ast-to-call-graph.md" "src/analysis/invalidation.rs"`: shows the retired files as deleted and no live `src/analysis/invalidation.rs` file.
- Linter check for this progress document: no diagnostics.
- No `cargo fmt`, `cargo check`, or full test suite was run because this slice changed only cleanup documentation and did not edit Rust code or behavior.

#### Iteration 26 Learnings

- SG-CLEAN-061 can focus on actual determinism and index integrity assertions; the stale-reference pass did not find a live standalone `CallGraph` schema, converter, module, public API, or CLI command to clean up first.
- Broad `old` and `legacy` searches are noisy in this repo because they also find unrelated fixture text, placeholder explanations, and non-call-graph compatibility contracts; future cleanup passes should classify by architectural ownership before rewriting wording.

### Iteration 27

- Focus: SG-CLEAN-061
- Subagent: launched
- Goal: verify direct `ProgramSupergraph` determinism and index integrity without reintroducing standalone call graph compatibility/projection assertions.
- Result: complete

#### SG-CLEAN-061 Result

- Strengthened the Python direct supergraph e2e test with a focused integrity helper that validates stable repeated output, sorted unique node and edge IDs, position indexes, node and edge kind indexes, source-span indexes, artifact/callable/owner indexes, and call indexes directly from `ProgramSupergraph` vectors.
- The call index assertions now reconstruct expected `calls_by_caller` and `call_site_to_calls` from all `Calls` edges, while reconstructing `calls_by_concrete_target`, `caller_to_concrete_target_calls`, and `caller_to_concrete_call_targets` only from `Calls` edges with callable/external concrete targets.
- The helper also checks concrete call edge targets resolve through the node-position index to `Callable` or `ExternalTarget` nodes, preserving current targetless unresolved/dynamic non-callable call semantics.

#### Iteration 27 Verification

- `cargo fmt`: passed.
- `cargo test --test python_incident_triage_e2e analyzes_incident_triage_python_fixture_into_supergraph_directly`: passed, 1 focused direct supergraph test.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo check`: passed.

#### Iteration 27 Learnings

- `ProgramSupergraphBuilder::finish()` still emits sorted node/edge vectors, then sorted/deduplicated index vectors; integrity tests should compare against that sorted index contract rather than raw edge traversal order.
- Direct construction already preserved targetless unresolved/dynamic calls in all-call caller and call-site indexes while excluding them from concrete-target indexes; SG-CLEAN-061 now has fixture-level regression coverage for that split.
- SG-CLEAN-062 can proceed to full verification with no known determinism or index-integrity blocker.

### Iteration 28

- Focus: SG-CLEAN-062
- Subagent: launched
- Goal: run final formatting, targeted integration tests, full test suite, final acceptance searches, and close the cleanup loop if verification passes.
- Result: complete

#### SG-CLEAN-062 Result

- Ran the final verification suite after SG-CLEAN-061 and confirmed all requested targeted and full tests pass.
- Verified the retired live paths are absent from the working tree: `src/call_graph.rs`, `src/call_graph/`, and `doc/ast-to-call-graph.md`.
- Final live reference search found no standalone `CallGraph` schema, converters, public analysis APIs, or CLI command implementations under `src`.
- Remaining matches are intentional: `CallGraphView` / `CallGraphFilter` logical view terminology, negative CLI help assertions for removed `call-graph-*` commands, and historical cleanup-plan/progress documentation.
- No additional JSON/schema changes were made in this verification slice.

#### Iteration 28 Verification

- `cargo fmt`: passed.
- `cargo test --test python_incident_triage_e2e`: passed, 8 tests.
- `cargo test --test typescript_incident_board_e2e`: passed, 6 tests.
- `cargo test --test cli_supergraph`: passed, 5 tests.
- `cargo test --test supergraph_language_parity`: passed, 1 test.
- `cargo test`: passed, including 133 lib tests, 0 main tests, 5 CLI integration tests, 8 Python e2e tests, 1 language parity test, 6 TypeScript e2e tests, and 0 doc tests.
- Live filesystem check confirmed `src/call_graph.rs: false`, `src/call_graph: false`, and `doc/ast-to-call-graph.md: false`.
- Live `rg` search for standalone call graph schemas, conversion functions, removed APIs, and removed CLI commands found no active `src` implementation matches; remaining test/doc matches were classified as intentional.

#### Iteration 28 Learnings

- The final cleanup state is fully supergraph-only: `ProgramSupergraph` is the only physical graph model, and call graph behavior is exposed as logical views over it.
- Final verification did not require representative JSON comparison because no code or schema output changed during SG-CLEAN-062 and the deterministic CLI/supergraph tests passed.
- The cleanup loop can stop; further action should be limited to human review or committing the accumulated changes if requested.

## Learnings

- The cleanup plan intentionally removes the standalone `CallGraph` physical model rather than preserving it behind compatibility shims.
- Each loop iteration should finish with this document updated before moving to the next SG-CLEAN item.
- Current baseline is clean and fast enough for targeted e2e plus full `cargo test` on this machine.
- A useful SG-CLEAN-001 safety net can assert `ProgramSupergraph` facts and `CallGraphView` indexes directly without starting the direct-lowering refactor.
- Several no-regression checks currently exist only through compatibility comparisons with `CallGraph`; later test rewrite phases should replace those with direct supergraph/view assertions.
- SG-CLEAN-010 can create the direct lowering owner without switching public builders; the behavior-preserving path should wait until adapter naming and direct call resolution are ready.
- SG-CLEAN-011 can move language adapter naming and call resolution result types to supergraph concepts while keeping old public behavior by converting only inside the bridge.
- SG-CLEAN-012 can insert base source facts directly through `ProgramSupergraphBuilder` while leaving call-site and `Calls` edge emission in pending state for SG-CLEAN-013.
- SG-CLEAN-013 can resolve calls directly through `ProgramSupergraphBuilder` without switching public builders; SG-CLEAN-014 should now preserve incoming local call counts and decorated callable metadata before SG-CLEAN-015 switches construction.
- SG-CLEAN-014 can preserve callable metadata as a named direct-lowerer finalization step; SG-CLEAN-015 should now focus on returning the direct builder output from public supergraph construction paths.
- SG-CLEAN-015 can switch public supergraph builders while leaving standalone call graph APIs untouched; deterministic direct supergraph tests should assert builder/index/view behavior rather than exact `CallGraph` compatibility projection equality.
- SG-CLEAN-016 can complete the builder-facing typed edge API while leaving finished-graph semantic enrichment on its current mutation helpers; `Defines` and `Uses` need explicit edge endpoints because their payloads are not full endpoint descriptors.
- SG-CLEAN-020 can preserve fixture call behavior with `CallGraphView` and direct `EdgeFact::Calls` checks; SG-CLEAN-021 should now focus on remaining compatibility equality/projection assertions rather than e2e standalone graph fixtures.
- SG-CLEAN-021 can finish e2e compatibility cleanup by asserting canonical `Calls` edge shape and call-view helper completeness directly; SG-CLEAN-023 remains the right place to remove internal `call_graph_from_supergraph` tests.
- SG-CLEAN-022 can keep CLI regression coverage centered on AST and supergraph JSON behavior while leaving old command implementation removal to SG-CLEAN-033.
- SG-CLEAN-023 can remove internal compatibility projections without reducing coverage by asserting `CallGraphView` traversal and canonical supergraph call indexes directly.
- SG-CLEAN-030 can remove public standalone call graph APIs while temporarily preserving old CLI commands through crate-internal builders; SG-CLEAN-031 should now focus on removing the private `CallGraph` schema.
- SG-CLEAN-031 can remove the private schema cleanly by making the old CLI bridge supergraph-backed; SG-CLEAN-032 should now focus on remaining conversion-function references and documentation cleanup.
- SG-CLEAN-032 can complete as verification after SG-CLEAN-031 because no whole-graph conversion functions, conversion-only helpers, or active code references remain; SG-CLEAN-033 should now remove the old CLI command names.
- SG-CLEAN-033 can remove the old CLI command names and temporary bridge without touching broader call graph adapter/lowering code; SG-CLEAN-034 should now move or delete the remaining `src/call_graph` module contents.
- SG-CLEAN-034 can complete by relocating adapter/language lowering under `analysis::source_graph`; after that, `call_graph` source/test references should be limited to intentional logical view terms until SG-CLEAN-040 decides final view naming.
- SG-CLEAN-035 can remove call preservation cleanly once unresolved calls are treated as call-site `ResolvesTo` facts to placeholder external targets; SG-CLEAN-040 kept the logical call graph view and finalized it as `CallGraphView`.
- SG-CLEAN-040 confirms `call_graph_view()` reads well beside the other logical `ProgramSupergraph` view accessors; SG-CLEAN-041 should focus only on call payload normalization.
- SG-CLEAN-041 can use targetless `Calls` edges for unresolved and dynamic non-callable calls while keeping placeholder/binding resolution detail in `ResolvesTo`; SG-CLEAN-042 should now focus on whether call index names describe this traversal model clearly.
- SG-CLEAN-042 can keep caller and call-site indexes as all-call traversal indexes, while naming callable/external-target indexes as concrete-target indexes to reflect targetless unresolved/dynamic calls.
- SG-CLEAN-050 should teach the direct `ProjectAst` -> `ProgramSupergraphBuilder` -> semantic enrichment -> logical views architecture first, with `CallGraphView` as the call graph surface rather than a migration outcome.
- SG-CLEAN-051 can preserve adapter and call resolution details in `doc/ast-to-supergraph.md` while removing standalone call graph persistence/schema guidance; SG-CLEAN-052 should now update the implementation checklist.
- SG-CLEAN-052 can keep `supergraph-todo.md` as an implementation checklist by rewriting stale baseline items around direct `analysis::source_graph` lowering and linking cleanup history to the dedicated plan/progress tracker; SG-CLEAN-053 should now handle broader learnings and index docs.
- SG-CLEAN-053 can keep broader learning history useful by updating only current-sounding guidance: call graph behavior should be described as `CallGraphView` / `CallGraphFilter` over `ProgramSupergraph`, with all-call and concrete-target indexes named explicitly.
- SG-CLEAN-060 confirms the live repository no longer contains standalone `CallGraph` code paths; remaining call graph names are logical view APIs or cleanup history, and SG-CLEAN-061 can proceed to determinism/index checks.
- SG-CLEAN-061 confirmed direct `ProgramSupergraph` construction preserves deterministic vector ordering and canonical indexes; final verification should watch for broad-suite regressions rather than a known index gap.
- SG-CLEAN-062 completed the cleanup loop with formatting, targeted integration tests, full suite verification, and final stale-reference checks all passing.
