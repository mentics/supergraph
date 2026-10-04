# Supergraph Implementation Learnings

## SG-010 Structural Node Families

- Added first-class node kinds and payloads for `Statement`, `Expression`, `Condition`, `Symbol`, `Definition`, `Use`, `Value`, `BasicBlock`, and `DomainKnowledge`.
- Kept spans, ownership, confidence, and evidence in the existing `GraphNode` envelope; structural payloads carry graph identity and semantic references such as callable, scope, symbol, value, child, and outcome IDs.
- Builder helpers accept explicit `SourceOwnership`, optional span, confidence, and evidence so later lowering stages can preserve provenance without inventing parallel metadata fields.
- Domain knowledge is modeled as a graph node with scope, source, status, and applies-to references; it remains non-deterministic analysis context for human-readable output, not a source of code facts.
- Tests run: `cargo test structural_node_families_are_constructible_with_envelope_metadata`; `cargo test`. Both passed.
- Follow-up: SG-011 should add parser/analysis version, content hash, syntax reference, original expression text, and normalized expression metadata where needed; SG-020 and SG-021 can now lower parser facts into these structural families.

## SG-011 Source And Provenance Metadata

- Added compact provenance fields to `Evidence`: source span, parser version, analysis version, content hash, and optional syntax reference. Runtime nodes still store references and small metadata, not parser ASTs or large source blobs.
- Added artifact-level optional `content_hash`, expression-level `original_text`, and `NormalizedExpression` payloads for canonical/operator/identifier/member/literal metadata.
- `ProgramSupergraphBuilder::refresh_provenance` backfills evidence from artifact ownership and is rerun after semantic enrichment so derived CFG, DFG, interprocedural, and requirement facts retain parser and analysis traceability.
- Tests run: `cargo test refresh_provenance_backfills_derived_fact_evidence`; `cargo test structural_node_families_are_constructible_with_envelope_metadata`; `cargo test`. All passed. One combined focused test command failed because `cargo test` accepts only one test-name filter; reruns passed separately.
- Follow-up: SG-012 can now separate stable subject identity from fact identity using these provenance fields without adding source or AST blobs to runtime facts.

## SG-012 Stable Subject And Fact Identity

- Kept `node_id` and `edge_id` as stable subject references for runtime traversal, indexes, callers, and requirements; added envelope-level `fact_id` and `payload_hash` for immutable versions of exact node and edge payloads.
- `refresh_fact_identity` runs after provenance refresh in builder finish and semantic enrichment, so payload, ownership, confidence, evidence, parser/content/analysis provenance, and endpoint changes create new fact IDs without changing subject IDs.
- Existing serialized graphs can omit the new fields during deserialization because `fact_id` and `payload_hash` default empty and are recomputed by graph refresh paths.
- Tests run: `cargo fmt`; `cargo test stable_subject_references_survive_callable_fact_payload_changes`; `cargo test structural_node_families_are_constructible_with_envelope_metadata`; `cargo test refresh_provenance_backfills_derived_fact_evidence`; `cargo test`. All passed.
- Follow-up: SG-013 can layer explicit uncertainty classifications onto payloads now that subject references and fact versions are separated; SG-112 should document serialized schema migration rules for `fact_id` and `payload_hash`.

## SG-013 Explicit Uncertainty

- Added envelope-level `uncertainty` for nodes and edges with classifications `Exact`, `Probable`, `Possible`, `External`, `Unresolved`, `Ambiguous`, `Stale`, and `Unsupported`; `Confidence` remains a strength signal while `uncertainty` records the fact status.
- `refresh_uncertainty` derives the classification from existing payloads where possible: resolution facts drive call and symbol status, diagnostics drive unresolved/ambiguous/unsupported/alias cases, domain knowledge status drives stale facts, and current approximation precision strings classify probable/possible flow edges.
- Stale domain knowledge and alias uncertainty are first-class diagnostic/status cases, so unresolved calls, ambiguous symbols, unsupported syntax, dynamic dispatch, alias uncertainty, and stale domain knowledge are retained as graph facts or diagnostics.
- Tests run: `cargo fmt`; `cargo test uncertainty_classifications_cover_sg013_acceptance_examples`; `cargo test structural_node_families_are_constructible_with_envelope_metadata`; `cargo test refresh_provenance_backfills_derived_fact_evidence`; `cargo test stable_subject_references_survive_callable_fact_payload_changes`; `cargo test`. All passed.
- Follow-up: SG-014 can index `uncertainty` if callers need fast filtering; future lowering tasks should set specific resolution/diagnostic payloads and rely on `refresh_uncertainty` rather than inventing parallel status fields.

## SG-014 Graph Indexes

- Extended `GraphIndexes` with deterministic ID position maps, source-span-to-node keys scoped by optional artifact ID, explicit artifact/callable ownership buckets, symbol-to-definition/use buckets, bidirectional requirement/code trace buckets, nested outgoing/incoming edge-family buckets, and uncertainty buckets.
- Kept `build_indexes` as the single construction point after sorting, provenance refresh, uncertainty refresh, and fact identity refresh; builder finish and semantic enrichment both continue rebuilding indexes from graph facts.
- Added small indexed helper paths for structural, call graph, PDG/SDG, requirements, and requirement sync traversal so common views can start from indexes instead of filtering the full graph.
- Tests run: `cargo fmt`; `cargo test sg014_indexes_common_view_traversals_without_full_graph_scans`; `cargo test structural_node_families_are_constructible_with_envelope_metadata`; `cargo test refresh_provenance_backfills_derived_fact_evidence`; `cargo test stable_subject_references_survive_callable_fact_payload_changes`; `cargo test uncertainty_classifications_cover_sg013_acceptance_examples`; `cargo test`. All passed.
- Follow-up: SG-020 can use the structural/span/ownership indexes while lowering parser facts into statement facts; SG-111 can later promote the helper paths into stable public view APIs.

## SG-020 Statement Lowering

- Added a semantic enrichment pass that lowers parser `StatementAst` facts into first-class `Statement` nodes before CFG/DFG enrichment, preserving callable/scope/artifact ownership, source spans, parser evidence, stable subject IDs, fact identity, uncertainty, and graph indexes through the existing refresh pipeline.
- Statement containment is span-derived and immediate-parent only: callable-to-top-level and statement-to-child `Contains` edges are emitted, sibling ordinals are source-ordered, and parent payloads carry ordered `child_statement_ids`. `expression_ids` remains empty until SG-021 lowers expressions.
- Normalized Python and TypeScript statement kinds now cover declarations, assignments, expressions, branches, loops, returns, raise/throw, function declarations, class declarations, and unknown statements without adding structured branch/loop CFG behavior.
- Tests run: `cargo fmt`; `cargo test sg020`; `cargo test`. All passed.
- Follow-up: SG-021 can attach expression nodes to these statement payloads; SG-022 can replace span-derived branch/loop containment with explicit structured region facts.

## SG-021 Expression Lowering

- Added an expression semantic enrichment pass between statement lowering and CFG/DFG enrichment. It lowers parser `ExpressionAst` facts into first-class `Expression` nodes, attaches root expression IDs to the nearest containing statement, and emits immediate `Contains` edges for statement-to-expression and expression-to-expression structure.
- Expression parentage and ordinals are span-derived like SG-020 containment; normalized payloads carry canonical text plus identifier, literal, member, and operator details for identifiers, literals, calls, fields, indexes, assignments, operators, conditional expressions, await/yield, and unknown expressions.
- Unknown expressions are retained as possible/uncertain facts instead of being dropped. SG-021 intentionally stops at expression facts; value nodes and data-flow value edges remain SG-023/SG-070 work.
- Tests run: `cargo fmt`; `cargo test sg021`; `cargo test`. All passed.
- Follow-up: SG-022 can use expression IDs for branch/loop/exception structure, and SG-023 can lower expression facts into value nodes for arguments, return values, assignment RHS values, fields, indexes, and call results.

## SG-022 Structured Region Lowering

- Added an SG-022 semantic pass after expression lowering and before CFG enrichment. It emits first-class `Condition` facts for branch, loop, and exception regions with explicit region membership, continuation points, fallthrough behavior, expression references, stable IDs, provenance, uncertainty refresh, fact identity, and indexes.
- Extended compact schema payloads instead of adding a new edge family: `Condition` now carries region facts (`branch-body`, `else-body`, `loop-body`, `try-body`, `catch-body`, `finally-body`), continuation metadata, and fallthrough behavior; `Statement` now carries control effects for normal fallthrough, break, continue, return, raise, throw, try, catch, and finally.
- Parser normalization now preserves `break`, `continue`, `try`, `catch`, and `finally` statement kinds for Python and TypeScript, and Python `for` loops produce loop condition facts. CFG enrichment now prefers SG-022 structured region facts and uses the old source-order/span approximation only as a fallback until SG-050 replaces CFG construction fully.
- Tests run: `cargo fmt`; `cargo test sg022`; `cargo test`. All passed.
- Follow-up: SG-050 can now consume explicit region facts to replace the remaining global source-order CFG sequencing; SG-023 remains dependency-ready for value lowering and should not need to reinterpret structured control metadata.

## SG-023 Value Lowering

- Added a dedicated value semantic pass after expression/structure lowering and before CFG/DFG/interprocedural enrichment. It emits role-specific `Value` nodes for formal parameters, receiver/self/this, call arguments, return values, call results, exceptional values, and caller-visible mutable argument state.
- Extended the compact `Value` payload with `ValueRole` plus optional call-site, name, ordinal, and state-of references so SG-023 can describe value roles without introducing value-instance data-flow or scope normalization.
- Interprocedural `ParameterIn`, `ParameterOut`, `ReturnsTo`, and `ThrowsTo` edges now use value-node endpoints: argument/receiver values to callee parameter/receiver values, return values to call-result values, exception values to call exceptional-result values, and parameter values to mutable argument state values.
- TypeScript call argument shapes can report identifier arguments as names; SG-023 treats total arity as positional fallback while still honoring true name matches where available.
- Tests run: `cargo fmt`; `cargo test sg023`; `cargo test emits_typescript_phase5_supergraph_facts --test typescript_incident_board_e2e`; `cargo test`. All passed.
- Follow-up: SG-070 should add real value-instance data flow between these value nodes; SG-080 can consume the value-to-value interprocedural endpoints without using call sites or CFG return nodes as substitutes.

## SG-024 Scope Normalization

- Added a scope semantic pass after structured region lowering. It normalizes existing module/class/function scopes, emits block, catch, comprehension, and nested declaration `Scope` facts, and adds `Contains` edges plus ownership/index retargeting for facts inside binding-relevant lexical scopes.
- Extended `Scope` with compact normalization metadata: `variant`, `language_variant`, and `binding_behavior`. Python control blocks are transparent binding scopes, Python except handlers are language-specific, Python comprehensions are binding boundaries, and TypeScript blocks/catch clauses are binding boundaries.
- Scope normalization keeps call-oriented traversal centered on callable/module/class ownership; block/catch/comprehension scopes support supergraph facts without becoming callable targets in `CallGraphView`.
- Tests run: `cargo fmt`; `cargo test sg024`; `cargo test`. All passed.
- Follow-up: SG-030 should consume `binding_behavior` when placing definitions/bindings, walking through transparent scopes and honoring language-specific handler/comprehension behavior instead of recomputing lexical scope rules from spans.

## SG-030 Lexical Symbol Tables

- Added a `symbols` semantic pass after scope normalization. It reuses parser-backed binding facts where their spans cover definition targets, emits missing lexical `Binding` facts, creates `Symbol`, `Definition`, and `Use` nodes, and connects them with `Binds`, `Defines`, `Uses`, and binding-level `ResolvesTo` facts.
- Binding placement now consumes normalized `ScopeBindingBehavior`: transparent scopes are walked through, boundary scopes retain block-local bindings, and language-specific handler scopes remain explicit binding scopes for later language-aware resolution.
- External import targets get placeholder `ExternalTarget` facts when needed; unresolved names are retained as `Symbol` facts with unresolved uncertainty instead of being dropped. Statically visible `self`/`this`/`cls` field receivers are represented as field symbols/bindings without resolving call sites beyond SG-030 placeholders.
- Tests run: `cargo fmt`; `cargo test sg030`; `cargo test`. All passed.
- Follow-up: SG-031 should resolve `Use`, `Symbol`, and call-site facts to bindings, callables, fields, external targets, ambiguous candidates, or unresolved targets without redoing symbol table construction.

## SG-031 Use, Symbol, And Call-Site Resolution

- Extended `ResolvesTo` payloads with explicit `Resolution`, and `refresh_uncertainty` now classifies resolution edges from that field instead of only from confidence.
- The symbol pass now emits `ResolvesTo` edges from `Symbol`, `Use`, and `CallSite` sources. Uses and symbols resolve through SG-030 lexical bindings and then through binding targets when available; field references target field bindings, callable/class/import targets retain concrete callable/scope/external targets, ambiguous lexical candidates keep one ambiguous edge per candidate, and unresolved facts get explicit placeholder targets.
- Call-site resolution emits `ResolvesTo` evidence for exact local, external, unresolved, and dynamic binding cases; canonical `Calls` edges are derived from those call-site resolutions in the later call semantic pass.
- Tests run: `cargo fmt`; `cargo test sg031`; `cargo test sg030`; `cargo test`. All passed.
- Follow-up: SG-032 can attach invalidation dependencies to these resolution edges; SG-040 can consume call-site `ResolvesTo` targets to produce first-class `Calls` facts and logical call graph traversal.

## SG-032 Resolution Invalidation Dependencies

- Invalidation is now modeled as a derived view rather than a persisted
  `Invalidates` edge family. The view derives resolution dirtying from artifacts,
  parser-backed statement/expression/definition/use/call-site facts, symbols,
  bindings, declaration/import facts, resolved targets, and existing semantic
  edges.
- Resolution-derived invalidation propagates to existing call facts,
  symbol/data-flow facts, interprocedural call facts, and generated requirements
  traced from affected call/data-flow facts. Stable node and edge subject IDs
  remain the invalidation surface.
- TypeScript exports continue to use existing declaration symbol/binding facts
  rather than a new export fact family. Call requirements are currently traced to
  `CallSite` nodes, so call-resolution inputs dirty those traced requirements
  through the derived closure.
- Tests run: `cargo fmt`; `cargo test sg032`; `cargo test`. All passed.
- Follow-up: SG-040 can consume these dependencies when deriving `Calls` facts from call-site resolutions; later invalidation graph work can add transitive closure/query helpers without changing the SG-032 edge convention.

## SG-040 Call-Site Calls Lowering

- Added a call-edge semantic pass immediately after SG-031 resolution. It derives canonical `Calls` facts from call-site `ResolvesTo` edges, with `source_id == call_site_id` and `target_id` populated only for concrete callable or external targets.
- Caller-oriented traversal remains in the `Calls` payload and indexes, while targetless unresolved and dynamic non-callable calls keep details in `ResolvesTo` evidence and `unresolved_target`. `CallGraphView` exposes indexed helpers over these supergraph facts.
- Invalidation treats concrete call edge targets and call-site resolution evidence as call-resolution inputs, so derived call/interprocedural facts can be dirtied without inventing placeholder call targets.
- Tests run: `cargo fmt`; `cargo test sg040`; `cargo test sg031`; `cargo test sg032`; `cargo test stable_subject_references_survive_callable_fact_payload_changes`; `cargo test sg014_indexes_common_view_traversals_without_full_graph_scans`; `cargo test`. All passed.
- Follow-up: SG-041 can normalize call endpoints/payloads while preserving indexed caller and call-site traversal. SG-042 remains responsible for TypeScript module-initializer call coverage.

## SG-041 Call Endpoint Normalization

- Normalized `Calls` edges as call-site-sourced supergraph facts: `source_id` is the `CallSite`, and `target_id` is present only for concrete callable or external targets.
- Targetless unresolved and dynamic non-callable calls remain discoverable through all-call caller and call-site indexes, with resolution detail preserved in `ResolvesTo` facts and `Calls::unresolved_target`.
- `CallGraphView` uses indexed caller-to-concrete-target and target-list helpers for concrete target traversal while all-call helpers retain unresolved and dynamic calls.
- Tests run: `cargo fmt`; `cargo test sg041_indexes_caller_to_target_summaries_without_legacy_call_edges`; `cargo test sg040`; `cargo test sg014_indexes_common_view_traversals_without_full_graph_scans`; `cargo test`. All passed.
- Follow-up: SG-042 can now add TypeScript module-initializer call coverage without changing the caller-to-target summary convention.

## SG-042 TypeScript Module-Initializer Calls

- TypeScript top-level call expressions, constructor calls, and uppercase JSX component tags are collected into `FileAst.calls` with `CallContext::ModuleInitializer`; traversal skips nested class/function/arrow/function-expression bodies so body calls remain owned by their callable.
- Reused the existing module initializer callable convention (`<module>` callable, e.g. `sample:<module>` as the subject ID) and the canonical call pipeline: parser call facts become `CallSite` nodes, `ResolvesTo` evidence, call-site-sourced `Calls` edges, uncertainty, provenance, stable identities, and all-call/concrete-target indexes without duplicate legacy call records.
- Focused tests cover TypeScript module top-level local, external/imported, unresolved, constructor/call, and JSX-style call capture where practical; existing Python module-initializer behavior continues through the same canonical call path.
- Tests run: `cargo fmt`; `cargo test sg042_typescript_parser_emits_module_initializer_calls`; `cargo test sg040_lowers_typescript_local_calls_and_keeps_caller_queries_compatible`; `cargo test sg041_indexes_caller_to_target_summaries_without_legacy_call_edges`; `cargo test sg040`.
- Follow-up: no SG-042 blocker found. SG-050 remains the next checklist phase and should replace source-order CFG lowering without changing SG-042 call ownership.

## SG-050 Structured Recursive CFG Lowering

- Replaced the callable-wide source-order CFG chain with structured recursive lowering over normalized `Statement` sibling lists and SG-022 `Condition` regions. Sequences now thread explicit open exits, branches lower true/else/fallthrough paths, loops route normal and `continue` exits back to the condition, and `break` exits flow to the loop continuation point.
- CFG construction keeps the existing compatible `ControlFlow` node and edge schema while using `Statement`, `Expression`, and `Condition` facts as inputs. Returns and raises/throws no longer fall through to later source-ordered statements; handled raises route to catch/finally regions where structured facts provide them.
- Short-circuit binary and conditional expression facts now create expression-control CFG nodes before their owning statement, preserving expression control regions without adding SG-053 branch outcome metadata.
- Tests run: `cargo fmt`; `cargo test sg050`; `cargo test sg022`; `cargo test sg040`; `cargo test sg041`; `cargo test sg042`; `cargo test`. All passed.
- Follow-up: SG-051 should split normal and exceptional callable exits so unhandled raises/throws, handled exceptions after `finally`, and callable returns no longer share the coarse compatibility exit node.

## SG-051 Normal And Exceptional Callable Exits

- Split callable CFG exits into stable `normal-exit` and `exceptional-exit` `ControlFlow` nodes while preserving the existing `Exit` role and compatibility helper convention; returns, ordinary fallthrough, break/continue fallbacks, and empty callables target the normal exit, while unhandled raise/throw exits target the exceptional exit.
- Kept handled exception lowering local to SG-050 structured regions: raises/throws inside a protected region still branch to catch/except handlers when present, and finally-only handlers route through cleanup while preserving the pending exceptional exit after cleanup fallthrough.
- Did not add SG-053 edge outcome metadata; exit meaning is represented by the target exit node identity and label/semantic kind, with existing provenance, uncertainty refresh, fact identity, and indexes preserved by the enrichment pipeline.
- Tests run: `cargo fmt`; `cargo test sg051`; `cargo test sg050`; `cargo test sg022`; `cargo test sg040`; `cargo test sg041`; `cargo test sg042`; `cargo test`. All passed.
- Follow-up: SG-052 can now add merge/basic-block nodes using separate terminal targets; SG-053 should add explicit branch/exception/finally outcome metadata so traversals can distinguish why paths enter shared cleanup or exit nodes.

## SG-052 Merge And Basic-Block Nodes

- Added first-class CFG merge nodes with `ControlFlowNodeRole::Merge`. Structured branch, loop-exit, finally-entry, and exception-region rejoins now materialize as stable `ControlFlow` nodes, and continuing paths flow through those joins before the next statement or callable exit.
- Added compact deterministic `BasicBlock` facts for straight-line statement runs. They are indexed structural facts with entry/exit CFG node references and do not replace or duplicate statement-level CFG edges.
- Kept SG-052 scoped away from SG-053: edges still use the existing `Sequential`, `Branch`, `LoopBack`, `Entry`, and `Exit` kinds without true/false/arm/finally outcome metadata. Switch/match arm-level joins remain limited by the current parser facts, which do not yet expose normalized arm regions.
- Tests run: `cargo fmt`; `cargo test sg052`; `cargo test sg05`; `cargo test`. All passed.
- Follow-up: SG-053 should attach outcome metadata to the new merge-entry edges; SG-054 remains dependency-ready for unreachable diagnostics over the explicit merge CFG.

## SG-053 Branch Outcome Metadata

- Extended `ControlFlow` edge facts with compact `outcome` metadata plus optional `branch_arm` labels/ordinals from structured regions. `flow_kind` remains the coarse compatibility traversal kind; `outcome` records why the successor is reachable (`true`, `false`, `fallthrough`, `exception`, `finally`, `break`, `continue`, `loop-back`, `return`, etc.).
- CFG lowering now carries branch-arm metadata through merge-entry edges so traversals can distinguish true/false branch rejoins, loop exits, catch/finally paths, break exits, continue edges, and ordinary fallthrough without inferring from target node labels.
- Existing serialized edges can omit the new fields because `outcome` defaults to `Unknown` and `branch_arm` defaults to absent; freshly emitted CFG edges should not use `Unknown`.
- Tests run: `cargo fmt`; `cargo test sg053`; `cargo test sg05`; `cargo test`. All passed.
- Follow-up: SG-054 can consume explicit outcomes to report unreachable statements without reinterpreting structured regions. Switch/match arm outcomes remain limited until parser facts expose normalized arm regions.

## SG-054 Unreachable Statement Diagnostics

- Added exact `UnreachableStatement` diagnostics from CFG statement-list lowering. A diagnostic is emitted when all pending incoming paths are terminal before the next sibling statement in the same structured region.
- Diagnostics relate the unreachable `Statement`/CFG node to the terminal statement facts and CFG nodes that blocked normal fallthrough. The CFG remains the source of truth: no normal sequential edge is added from terminal return/raise/throw/break/continue statements to the unreachable sibling.
- Terminal causes are carried explicitly for return, raise/throw, break, continue, and handled exception/loop continue cases where no open exit is propagated, so SG-054 does not reinterpret region syntax or start dominator work.
- Tests run: `cargo fmt`; `cargo test sg054`; `cargo test sg05`; `cargo test`. All passed.
- Follow-up: SG-060 is now dependency-ready for dominator/post-dominator analysis over the reachable CFG; switch/match arm-level unreachable precision remains limited by parser region facts.

## SG-060 Dominator And Post-Dominator Analysis

- Added reusable callable-local CFG extraction and dominance APIs in `analysis::control_dependence`: `callable_cfg` returns deterministic predecessor/successor maps over existing `ControlFlow` nodes and edges, and `analyze_callable_dominance` returns dominator and post-dominator sets plus immediate dominator maps.
- Dominator analysis is scoped to nodes reachable from the callable entry, so SG-054 unreachable CFG nodes remain visible in `CallableCfg::node_ids` but do not pollute dominance or post-dominance sets. Post-dominance uses reachable normal/exceptional exits as terminal roots and excludes unreachable terminal fragments.
- SG-060 intentionally does not persist graph facts and does not replace the current containment-based `Controls` emission; SG-061 can consume the computed CFG, successors/predecessors, `dominates`, `post_dominates`, and immediate dominator helpers to derive real `Controls` edges.
- Tests cover direct CFG fixtures for branches, loops, merge nodes, returns/normal exits, raises/exceptional exits, handled exceptions, and unreachable nodes excluded from analysis.
- Tests run: `cargo fmt`; `cargo test sg060`; `cargo test sg05`; `cargo test`. All passed.
- Follow-up: SG-061 should replace the old syntactic containment `Controls` approximation with post-dominance-based control dependence edges while preserving CFG reachability conventions.

## SG-061 Post-Dominance Controls Edges

- Replaced the syntactic containment `Controls` approximation with exact CFG/post-dominance-derived control dependence. The pass removes prior `Controls` facts, refreshes indexes after CFG/DFG emission, and emits only `sg061-post-dominance-control-dependence` edges.
- A CFG condition controls facts reached through a branch successor when that successor does not post-dominate the condition and the candidate target post-dominates that successor without post-dominating the condition. Branch merges and statements reached on all outcomes are therefore not controlled by the condition.
- Controlled CFG statements are projected to their corresponding `Statement` fact plus directly executing expression, call-site, and definition facts whose nearest CFG statement is that statement; nested branch contents are left to their own CFG nodes. Return, raise/throw, and nested condition CFG nodes remain direct controlled targets so requirement generation keeps condition/terminal leaves.
- Focused Python and TypeScript tests cover if/else true and false dependence, definitions and call sites, branch merges that both paths execute, loops, returns, exceptional raise/throw paths, nested branch control, and the absence of old containment precision.
- Tests run: `cargo fmt`; `cargo test sg061`; `cargo test sg06`; `cargo test`.
- Follow-up: SG-062 can now summarize path conditions from exact `Controls` facts instead of reconstructing containment or branch reachability.

## SG-062 Path Condition Summaries

- Added defaulted `Requirement.path_conditions` records with compact graph-backed references to the controlling CFG condition, structured `Condition` fact, expression fact, CFG branch outcome, optional branch arm, and secondary human-readable summary text.
- Requirement generation derives summaries from existing exact `Controls` facts plus SG-053 CFG outcome edges. Nested behavior inherits transitive path conditions through controlled condition CFG nodes; unconditional behavior keeps an empty path-condition list and unchanged prose.
- Leaf requirements for guarded definitions/assignments, call sites and side effects, returns, and raises/throws append deterministic path-condition prose while preserving trace edges to the controlled code fact and `Conditions` edges between requirement nodes.
- Tests run: `cargo fmt`; `cargo test sg062`; `cargo test sg06`; `cargo test`. All passed.
- Follow-up: SG-070 is dependency-ready for Phase 7 value-instance data flow; SG-091 can later turn these summaries into richer condition/guarded-behavior requirement hierarchy without recomputing control dependence.

## SG-070 Value-Instance Data Flow

- Extended value lowering so expressions that SG-023 did not already claim now get value instances: identifiers, assignments, operators, fields, indexes, await/yield/conditional expressions, unknown expressions, and computed expression values. Definitions and uses are backfilled with value IDs when a direct expression/parameter value is available.
- Added SG-070 `DataFlow` edges between `Value` node IDs for direct expression operands, assignment RHS-to-destination values, nearest-prior definition-to-use values, call argument values to call result values, return expression values to return values, and field/index base/index values to uncertain access values.
- Merge and loop-carried value facts are explicit uncertain `ValueKind::Merge` placeholders. They intentionally do not implement CFG-aware reaching definitions, precise branch merges, loop-carried recurrence, or field/index alias modeling; those remain SG-071 through SG-073 work.
- Tests run: `cargo fmt`; `cargo test sg023`; `cargo test sg070`; `cargo test sg06`. All passed.
- Follow-up: SG-071 should consume these value-instance endpoints to compute CFG-aware reaching definitions without relying on compatibility `DataFlow` nodes.

## SG-071 CFG-Aware Reaching Definitions

- Replaced nearest-prior same-name value-instance definition/use flow with callable-local forward reaching-definition analysis over the structured CFG. Parameters seed the callable entry; definitions kill prior definitions of the same name at their CFG node; uses consume the fixed-point `IN` set of their smallest executable CFG node.
- SG-071 emits only value-to-value `DataFlow` edges with precision `sg071-cfg-aware-reaching-definition`, preserving SG-070 expression operand/call/return/field/index flows and avoiding duplicate compatibility same-name `DataFlow` node edges.
- Branch alternatives, merge uses, loop backedges, break/continue exits, return-terminal paths, and unreachable definitions are covered by focused Python and TypeScript fixtures. SG-071 does not add explicit merge or loop-carried value nodes beyond the existing SG-070 placeholders.
- Tests run: `cargo fmt`; `cargo test sg07`. Both passed.
- Follow-up: SG-072 can replace multi-definition reaching sets at branch joins and loop heads with explicit merge/loop-carried value facts; SG-073 remains responsible for field/index alias precision.

## SG-072 Merge And Loop-Carried Values

- Reused SG-071's callable-local reaching-definition fixed point as the source of truth for explicit value merge facts. CFG merge nodes now get one named `ValueKind::Merge` fact per variable only when multiple value-backed definitions reach that join; single-definition paths do not create merge values.
- SG-072 emits uncertain value-to-value `DataFlow` edges with `DataFlowKind::MergeValue` and precision `sg072-branch-merge-value`: reaching definition values flow into the merge value, and the merge value flows into immediate post-merge uses when that reaching set feeds the use.
- Loop condition nodes with loop-back/continue predecessors now get named `loop-carried:<name>` merge values when backedge definitions participate in a multi-definition reaching set. `DataFlowKind::LoopCarriedValue` edges connect initial/backedge definitions to the loop-carried value and from that value to later-iteration and loop-exit uses.
- Tests run: `cargo fmt`; `cargo test sg07`.
- Follow-up: SG-073 remains responsible for field/property/index alias modeling; SG-072 intentionally only handles named value instances already exposed by SG-070/SG-071.

## SG-073 Fields, Indexes, And Aliases

- Added SG-073 access summaries over existing `Value` endpoints: assigned RHS values flow into field/property and index write values, same object/member or object/index writes flow to later reads, and possible same-member alias reads get uncertain summary edges instead of exact claims.
- Field/property keys use parser-backed object/member facts; index keys use parser-backed object/index facts when both sides have simple stable subjects. Dynamic, unsupported, or cross-base same-member cases emit `AliasUncertainty` diagnostics and keep possible edges at unknown confidence.
- Kept SG-074 out of scope: tests verify forward and backward traversal at the existing `DataFlow` edge level rather than adding slicing helper APIs.
- Tests run: `cargo fmt`; `cargo test sg07`.
- Follow-up: SG-074 can build public slice helpers over these edges; SG-083 can use SG-073 alias uncertainty to avoid overbroad parameter-out mutation summaries.

## SG-074 Forward And Backward Slices

- Added `ProgramDependenceGraphView` slicing helpers over existing facts: `value_forward_slice` traverses value-to-value `DataFlow` edges, and behavior helpers (`return_backward_slice`, `write_backward_slice`, `call_backward_slice`, `branch_backward_slice`) traverse backward from behavior facts to influencing values plus controlling conditions.
- Backward behavior slices bridge first-class facts to existing SG-061 projections by source span and callable ownership, so callers can start from returns, writes/definitions, call sites, branch conditions, or value facts without adding new analysis facts.
- Slice results return stable subject IDs for values, controlling condition CFG nodes, traversed edges, path-condition summaries, and related diagnostics; provenance, evidence, confidence, uncertainty, and fact identity remain on the referenced graph facts.
- Focused Python and TypeScript tests cover value-forward flow, returns, writes, external call sites, branch conditions, field/index flows, and alias-influenced uncertain flows.
- Tests run: `cargo fmt`; `cargo test sg074`.
- Follow-up: SG-080 remains dependency-ready for interprocedural value edges; SG-085 can later extend these intraprocedural slices across call boundaries.

## SG-080 Actual-To-Formal Parameter-In Edges

- `ParameterIn` now maps caller `ValueRole::Argument` nodes to callee `ValueRole::FormalParameter` nodes from canonical `Calls` facts, using exact confidence for exact local targets and possible uncertainty for possible local targets.
- Argument matching is named-first when a reported argument name matches a formal, then positional. TypeScript identifier argument names that do not match any formal fall back to positional mapping, preserving current parser facts without treating identifiers as true named parameters.
- External, unresolved, unsupported, and possible dynamic non-callable targets do not invent formal value nodes; they retain call facts and emit diagnostics tied to the call site/call edge when actual values cannot be connected to formals.
- Tests run: `cargo fmt`; `cargo test sg080`; `cargo test emits_typescript_phase5_supergraph_facts --test typescript_incident_board_e2e`; `cargo test`. All passed. One full-suite run failed before the TypeScript identifier-name fallback fix; reruns passed.
- Follow-up: SG-081 should move receiver/self/this flow out of the older SG-023 receiver summary convention and make receiver-to-formal edges explicit under its own rules.

## SG-081 Receiver-To-Self/This Parameter-In Edges

- Receiver flow is now an explicit SG-081 `ParameterIn` convention: caller `ValueRole::Receiver` nodes flow to callee `ValueRole::Receiver` nodes only when both the `CallSite` dispatch and local callable kind are method or constructor. Ordinary function calls still use the SG-080 argument-to-formal rules and do not receive invented receiver edges.
- Exact local receiver targets use `sg081-receiver-value-to-self-this-formal-value`; possible/probable/ambiguous local targets use `sg081-possible-call-target-receiver-value-to-self-this-formal-value`, which refreshes to possible uncertainty through the existing precision convention. External, unresolved, dynamic non-callable, missing call-site receiver, and missing callee receiver facts produce diagnostics instead of synthetic endpoints.
- Current constructor support is fact-driven: when lowering exposes a constructor call receiver/new-object value and a callee `this` receiver value, SG-081 connects them; otherwise it records the missing receiver value for later parser/lowering follow-up.
- Tests run: `cargo fmt`; `cargo test sg081`; `cargo test sg080`. Both passed.
- Follow-up: SG-082 can build return-to-call-result summaries next without changing receiver-to-self/this `ParameterIn` edges. SG-083 should later remove or narrow the older broad `ParameterOut` mutation summaries.

## SG-082 Return-To-Call-Result Summaries

- `ReturnsTo` now maps callee `ValueRole::ReturnValue` nodes to caller `ValueRole::CallResult` nodes from canonical `Calls` facts. Exact local targets use `sg082-callee-return-value-to-call-result-value`; possible/probable/ambiguous local targets use `sg082-possible-call-target-callee-return-value-to-call-result-value` and refresh to possible uncertainty.
- External, unresolved, unsupported, dynamic non-callable targets, missing callee return values, and missing caller call-result values emit diagnostics tied to the call site and call edge instead of inventing return or result endpoints.
- `ProgramDependenceGraphView` value slices treat `ReturnsTo` as value flow alongside local `DataFlow`, so backward slices from call result values reach callee returns and forward slices from callee returns continue into caller uses.
- Tests run: `cargo fmt`; `cargo test sg082`; `cargo test sg08`. Both passed.
- Follow-up: SG-083 can now narrow `ParameterOut` mutation summaries without changing SG-080/SG-081/SG-082 call-boundary conventions.

## SG-083 Parameter-Out Mutation Summaries

- `ParameterOut` now summarizes only callee receiver/formal mutations backed by SG-073 field/index write facts, mapping the mutated callee value back to the caller-visible mutable argument state from SG-080/SG-081 conventions.
- Direct field and index writes use exact SG-083 precisions; SG-073 possible alias summaries produce possible `ParameterOut` facts for alias-related receiver/formal values instead of broad default outputs. Missing caller state for a known mutation emits an SG-083 diagnostic rather than inventing an edge.
- Removed the old broad/default parameter-out convention and updated stale SG-023/e2e expectations so no-mutation calls no longer require `ParameterOut`.
- Tests run: `cargo fmt`; `cargo test sg083`; `cargo test sg08`; `cargo test sg023_lowers_python_values_and_interprocedural_value_edges`; `cargo test emits_python_phase5_supergraph_facts --test python_incident_triage_e2e`; `cargo test emits_typescript_phase5_supergraph_facts --test typescript_incident_board_e2e`; `cargo test`. All passed.
- Follow-up: SG-084 can model exceptional interprocedural flow next. Richer assignment-based aliasing from formals into locals remains future alias-analysis work; SG-083 currently consumes SG-073 field/index/alias facts only.

## SG-084 Exceptional Interprocedural Flow

- `ThrowsTo` now carries defaulted target metadata (`CallExceptionalValue`, `Handler`, `CallerExceptionalExit`, `CalleeExceptionalExit`) plus best-effort exception value/type text. Exact local exception constructors such as `ValueError()` / `new Error()` record a type; dynamic values keep uncertainty rather than claiming a type.
- The interprocedural pass classifies callee raised/thrown values with existing CFG exception facts: callee-handled raises flow to handler CFG nodes only, while unhandled raises flow to the callee exceptional exit, the caller call-exception value, and then either a caller catch/except handler or caller exceptional exit.
- External, unresolved, unsupported, missing summary, and missing caller exceptional CFG destinations emit SG-084 diagnostics instead of synthetic precise exception flow. Possible/probable call targets refresh `ThrowsTo` uncertainty to `Possible`.
- `ProgramDependenceGraphView` value slices include value-to-value `ThrowsTo` edges so call-exception values can trace back to callee raised/thrown values; SG-085 remains responsible for full SDG traversal across handler/exit endpoints.
- Tests run: `cargo fmt`; `cargo test sg084`; `cargo test sg08`. Both passed.
- Follow-up: SG-085 can now build cross-call SDG traversal over `ParameterIn`, `ReturnsTo`, `ParameterOut`, and `ThrowsTo`, including non-value handler and exceptional-exit endpoints.

## SG-085 SDG Traversal View

- Completed `SystemDependenceGraphView` as a traversal API over PDG edges plus `Calls`, `ParameterIn`, receiver `ParameterIn`, `ReturnsTo`, narrowed `ParameterOut`, and `ThrowsTo`. Traversals return stable node/edge subjects and keep provenance, evidence, fact identity, confidence, uncertainty, indexes, and diagnostics on referenced graph facts rather than copying metadata.
- Value slices cross call boundaries in both directions and preserve non-value `ThrowsTo` endpoints such as handler CFG nodes and normal/caller exceptional exits. Requirement slices start from `TracesTo`, traverse requirement relationships plus SDG edges, and bridge value nodes back to directly related expression/call/definition/use facts where existing requirement traces support it.
- Added helper APIs for entrypoint candidate discovery, external/unresolved sink discovery, entrypoint-to-sink call paths, cross-call slices, requirement slices, and transitive caller affectedness from callee changes using existing call and interprocedural summary edges.
- Focused Python and TypeScript fixtures cover argument-to-parameter, return-to-call-result, parameter-out mutation, exception handler/exit traversal, entrypoint-to-sink traversal, caller impact, and requirement slices across callable boundaries.
- Tests run: `cargo fmt`; `cargo test sg085`; `cargo test sg08`; `cargo test`. All passed.
- Follow-up: SG-090 is now dependency-ready to generate boundary and entrypoint requirements from the SDG view without adding new SG-085 facts.

## SG-090 Boundary And Entrypoint Requirements

- Requirement generation now refreshes indexes and consumes `SystemDependenceGraphView::entrypoint_candidates`, `sink_candidates`, and `entrypoint_to_sink_slice` to emit high-level boundary and entrypoint requirements from existing graph evidence rather than adding new fact families.
- Boundary requirements trace to artifact facts; entrypoint requirements trace to callable facts. Entrypoint prose is classified from existing deterministic evidence for module initializers, decorated route-like callables, component-like TypeScript callables, tests, config initializers, exported/root callables, and SDG-discovered external or unresolved sink paths.
- Focused Python and TypeScript fixtures cover module initializer, route, exported callable, component, test, config-like boundary candidates, trace edges from requirements to artifact/callable evidence, and deterministic summaries for entrypoints with external/unresolved sink paths.
- Tests run: `cargo fmt`; `cargo test sg090`.
- Follow-up: SG-091 can build guarded-behavior hierarchy from existing path-condition summaries; SG-092 remains responsible for expanding deterministic leaf behavior requirements.

## SG-091 Condition And Guarded Behavior Requirements

- Requirement generation now marks CFG condition requirements with the `sg091-condition` rule and traces each condition requirement to the controlling CFG node plus available structured `Condition` and predicate `Expression` facts.
- Guarded behavior requirements use existing SG-062 path-condition summaries and the `sg091-guarded-behavior` rule; `Conditions` edges are derived from full path-condition lists so nested guards connect directly to guarded calls, returns, raises/throws, definitions, and guarded field/index writes.
- Kept SG-091 scoped to hierarchy and guard prose: existing call/return/raise/definition requirement families remain in use, guarded external or unresolved calls are worded as side-effect/external targets, and SG-092 remains responsible for broader deterministic leaf behavior expansion.
- Tests run: `cargo fmt`; `cargo test sg091`; `cargo test sg09`; `cargo test`. All passed.
- Follow-up: SG-092 can expand deterministic leaf behavior coverage using the SG-091 guard hierarchy instead of recomputing control dependence or path conditions.

## SG-092 Leaf Behavior Requirements

- Requirement generation now emits deterministic code-backed leaves for first-class assignment/field-write definitions, literal constants, assignment expressions, field/index writes and mappings, return/exception/value leaves, emitted-event-like external calls, structural statements, and basic-block facts without adding new node or edge families.
- Guarded write leaves keep the SG-091 source-rule convention and path-condition text; SG-092-specific leaves use `sg092-leaf-behavior`, `sg092-guarded-leaf-behavior`, or `sg092-structural-fact` and trace to related value, expression, symbol, call-site, and child-expression facts where those graph facts already exist.
- Event-like wording is intentionally conservative and name-backed (`emit`, `dispatch`, `publish`, `notify`, `send_event` patterns): it still describes the call as a side-effect/external target and does not infer event payload semantics.
- Tests run: `cargo fmt`; `cargo test sg092 --lib`; `cargo test sg09 --lib`; `cargo test`. All passed.
- Follow-up: SG-093 can add meaningful `Orders`/`DecomposesTo` traversal from boundary behavior to these implementation leaves; richer event payload/domain naming remains SG-095+ work.

## SG-093 Ordering And Decomposition Relationships

- Requirement generation now emits `DecomposesTo` edges from conditions to guarded requirements alongside `Conditions`, from structural statements/expressions/basic blocks/definitions to code-backed child leaves, and from local call-site requirements to callee implementation requirements using existing `Calls` evidence.
- `Orders` edges now use semantic graph facts instead of incidental document order: CFG `ControlFlow` edges provide executable sequencing, and structural child lists, condition regions, and basic-block statement lists provide local ordering only where the graph records meaningful order.
- `RequirementGraphView` has indexed helpers for `DecomposesTo`, `Conditions`, and ordered successors so callers can traverse requirement hierarchy and sequence without scanning raw edges.
- Tests run: `cargo fmt`; `cargo test sg093 --lib`; `cargo test sg09 --lib`. Both focused suites passed.
- Follow-up: SG-094 can factor shared behavior now that multiple callers can decompose into existing callee implementation requirements; SG-100 should harden trace completeness without changing the SG-093 relationship conventions.

## SG-094 Shared Behavior Requirements

- Requirement generation now emits `sg094-shared-behavior` callable requirements for exact/probable local callees reused by multiple call sites across multiple callers, and caller call requirements decompose to that stable shared node instead of duplicating direct links to every callee leaf.
- Shared callable requirements trace to the callee callable and each participating call site, then decompose to the existing callee implementation requirements. SG-094 does not factor external, unresolved, unsupported, or uncertain dynamic behavior into broad deterministic claims.
- Nested guarded behavior with multiple graph-backed path conditions gets an `sg094-shared-guarded-behavior` wrapper that traces to the controlled code fact and its condition/expression evidence, is conditioned by each guard, and decomposes to the single original guarded leaf requirement.
- Tests run: `cargo fmt`; `cargo test sg094 --lib`; `cargo test sg09 --lib`; `cargo test`. All passed.
- Follow-up: SG-095 can improve shared requirement wording with domain knowledge without changing deterministic code facts; SG-100 should harden trace completeness across generated shared wrappers and existing leaves.

## SG-095 Domain Knowledge Prose Context

- Added `DependsOnDomainKnowledge` as a requirement-to-domain-knowledge edge family for human-readable prose dependencies. It is intentionally separate from `TracesTo`, so domain knowledge never becomes deterministic code evidence.
- Requirement generation now applies existing `DomainKnowledge` facts by `scope` and `applies_to` references to artifacts/modules, callables, statements, values, and requirements. Active knowledge appends `Domain context`, unknown knowledge appends `Possible domain context`, and stale/superseded knowledge appends a stale caveat plus a `StaleDomainKnowledge` diagnostic.
- SG-095 mutates only requirement titles/summaries and prose dependency edges; requirement IDs, code-backed graph facts, call/data/control facts, deterministic acceptance claims, and existing `TracesTo` edges remain unchanged by domain wording.
- Tests run: `cargo fmt`; `cargo test sg095 --lib`; `cargo test sg09 --lib`; `cargo test`. All passed.
- Follow-up: SG-100 should harden trace completeness for generated requirements without treating `DependsOnDomainKnowledge` as code evidence; SG-101 can later add invalidation dependencies from domain knowledge to generated prose views.

## SG-100 Requirement Trace Completeness

- Added a trace-completion pass after requirement relationships are emitted and before domain prose integration. Generated requirements now retain their primary code fact trace and gain supplemental `TracesTo` edges to graph-backed path-condition CFG, structured `Condition`, and predicate `Expression` facts when guarded prose depends on them.
- `RequirementGraphView` now exposes indexed requirement-to-code facts, locatable code facts, and reverse code-fact-to-requirement traversal. Locatable code evidence is a non-requirement, non-domain, non-diagnostic graph fact with a source span or a structural anchor such as an artifact, scope, or callable.
- Added `UntraceableRequirement` diagnostics for generated requirements that lack any locatable code-backed trace. `DependsOnDomainKnowledge` remains prose-only and is deliberately excluded from code trace indexes and reverse code lookup.
- Tests run: `cargo fmt`; `cargo test sg100 --lib`; `cargo test sg09 --lib`; `cargo test`. All passed.
- Follow-up: SG-101 can consume the completed `TracesTo` coverage plus the
  derived invalidation view when expanding dirtying dependencies; SG-102 can use
  the locatable trace view for edit classification.

## SG-101 Complete Invalidation Graph

- Replaced materialized invalidation edges with a derived view. Artifact changes
  now dirty artifact-owned parser outputs, semantic facts, analysis nodes, and
  derived edges; parser-backed same-span facts dirty related semantic facts;
  endpoint facts dirty derived edges; requirement relationship edges dirty
  generated requirement views through closure traversal.
- Requirement invalidation now consumes SG-100 `TracesTo` coverage and SG-093/SG-094 relationship edges. Code fact changes dirty traced requirements and generated trace/relationship edges, while `DependsOnDomainKnowledge` changes dirty only prose-dependent requirements, domain dependency edges, and stale-domain diagnostics without becoming code evidence.
- `RequirementGraphView::invalidation_closure` returns the transitive dirty set
  from the derived invalidation view, preserving stable subject/edge IDs as
  recomputation subjects and keeping artifact/domain invalidation scoped to
  related facts.
- Tests run: `cargo fmt`; `cargo test sg101 --lib`; `cargo test sg032 --lib`; `cargo test`. All passed.
- Follow-up: SG-102 is dependency-ready for requirements-to-code edit classification using SG-100 locatable traces plus SG-101 dirty closure. SG-105 can add concrete generated document nodes/views if persisted generated documents need their own recomputation subjects.

## SG-102 Requirements-To-Code Edit Classification

- Extended the read-only sync planner into a graph-backed classifier that preserves the existing `plan_requirement_to_code_sync` entry point while returning enriched evidence and affected-slice payloads for deterministic single-span, deterministic equivalent multi-span, structural, ambiguous, and unsupported outcomes.
- Classification now uses SG-100 trace candidates, requirement source spans, expression/symbol/binding/value evidence, path-condition summaries, SG-074/SG-085 slices, SG-101 invalidation closure, uncertainty, diagnostics, and source ownership boundaries. A requirement's own exact span wins over supplemental guard traces when it identifies one editable target; structural guard facts remain evidence rather than forced edit targets.
- Focused SG-102 fixtures cover Python single-span literal/assignment edits with path conditions and symbol/binding reporting, TypeScript equivalent multi-span constants, structural call-shape changes, ambiguous possible/dynamic traces, unsupported external/unowned targets, affected slices, and formatting/source ownership boundaries.
- Tests run: `cargo fmt`; `cargo test sg102 --lib`; `cargo test --lib`; `cargo test --test python_incident_triage_e2e`; `cargo test --test typescript_incident_board_e2e`. All passed.
- Follow-up: SG-103 is dependency-ready to execute only the deterministic single-span and equivalent multi-span classifications; SG-104 can consume structural classifications later for graph-backed generation plans.

## SG-103 Deterministic Code Edit Execution

- Added a scoped execution API alongside the read-only planner: `execute_requirement_to_code_sync` consumes SG-102 classifications plus caller-supplied artifact source text, and applies only deterministic single-span or deterministic equivalent multi-span replacements.
- SG-103 edits are byte-span bounded and UTF-8 boundary checked. They preserve all source text outside target spans, reject overlapping/out-of-bounds spans, missing artifact sources, and all structural, ambiguous, unsupported, external/unowned, or formatting-boundary-blocked classifications.
- Edit results return applied source edits, updated artifact source text, changed code facts/artifacts, and an explicit reanalysis plan built from SG-101 invalidation closure dirty IDs. There is no separate reanalysis runner yet, so callers receive the dirty subjects that must be recomputed.
- Tests run: `cargo fmt`; `cargo test sg103 --lib`; `cargo test --lib`; `cargo test --test python_incident_triage_e2e`; `cargo test --test typescript_incident_board_e2e`; `cargo test`. All passed.
- Follow-up: SG-104 can now consume structural classifications for generation plans; SG-105 remains responsible for generated requirement document/view regeneration after dirty subjects are recomputed.

## SG-104 Structural Code Generation Plans

- Added read-only `plan_structural_requirement_to_code_generation` output alongside SG-102/SG-103 sync APIs. It wraps the existing classification with deterministic structural generation actions, candidate/anchor spans, affected graph facts, ambiguity points, risks, and SG-101 dirty/reanalysis subjects without mutating source.
- Plans distinguish new code, moved code, new declarations, control-flow changes, data-flow/return-shape changes, ambiguous target selection, unsupported external/domain-only evidence, and ownership/formatting blockers. `Condition` facts are now structural edit evidence so guarded/control-flow requirements do not fall through to direct span execution.
- Focused tests cover Python control-flow plans, TypeScript declaration plans, return-shape/data-flow plans, moved-code-like basic blocks, ambiguous dynamic targets, unsupported external plus ownership blockers, affected facts, ambiguity points, dirty-set reporting, and SG-103 deterministic execution preservation.
- Tests run: `cargo fmt`; `cargo test sg104 --lib`. Both passed.
- Follow-up: SG-105 can consume SG-101 dirty subjects after structural generation or deterministic edits to regenerate requirement views; public API/CLI exposure remains SG-110+.

## SG-105 Human-Readable Requirement View Regeneration

- Added derived, in-memory requirement document generation on `RequirementGraphView`: `generated_requirement_document` renders the full graph view, and `regenerate_requirement_document` renders the SG-101 dirty subset from supplied dirty subject IDs.
- Generated artifacts include Markdown plus structured rows with requirement ID, depth, source rule, path conditions, trace metadata, relationship edge IDs, domain knowledge dependencies, diagnostics, dirty markers, and `canonical_state: false`. No generated document nodes or persisted canonical document facts are introduced.
- Ordering is deterministic from SG-093 `Orders` edges within `DecomposesTo` hierarchy, with stable ID fallback for disconnected or cyclic fragments. Domain knowledge appears only through existing prose and `DependsOnDomainKnowledge` metadata.
- Tests run: `cargo fmt`; `cargo test sg105 --lib`. Both passed.
- Follow-up: SG-110/SG-111 can expose these renderers publicly; SG-126 can add end-to-end sync/invalidation tests around regenerated views.

## SG-110 Public Supergraph CLI Commands

- Added `req-graph supergraph-python <path>` and `req-graph supergraph-typescript <path>` commands with the existing `--pretty` JSON option convention. Both commands reuse `analyze_python_supergraph` / `analyze_typescript_supergraph`, so CLI output is the full `ProgramSupergraph` from the same parser, direct supergraph lowering, semantic enrichment, requirement, and invalidation pipeline as library callers.
- Kept command behavior language-specific and conservative: file inputs must have `.py` for Python or `.ts`/`.tsx` for TypeScript, while directory inputs continue to use the existing language-specific discovery rules. AST and supergraph CLI commands share the same JSON printer convention.
- `GraphIndexes.source_span_to_nodes` now serializes its structured `SourceSpanIndexKey` as a deterministic string key and deserializes back to the same public struct, allowing `ProgramSupergraph` including indexes to be emitted as valid JSON.
- Tests run: `cargo fmt`; `cargo test --test cli_supergraph`; `cargo test`. All passed.
- Follow-up: SG-111 can expose stable public view APIs on top of the same graph; SG-112 should document the JSON map-key convention for `SourceSpanIndexKey` alongside broader schema migration rules.

## SG-111 Public View APIs

- Added top-level `ProgramSupergraph` view constructors plus stable `node`/`edge` lookups, so callers can enter structural, call graph, CFG, DFG, PDG, SDG, requirements, traceability, and invalidation views without scanning raw graph vectors.
- Kept public APIs as thin indexed wrappers over existing facts and traversal helpers: structural ownership/source-span/containment, call-site and caller-to-target calls, CFG callable nodes/edges/entry/exit/successors/predecessors, DFG callable/value/slice queries, traceability requirement-to-code and code-to-requirement, and invalidation closure all return stable subject or edge references.
- Tests use Python and TypeScript public views for structural, call graph, CFG,
  DFG, PDG, SDG, requirements/document generation, traceability, and
  invalidation queries. Fixtures now rely on canonical traces, ownership, and
  dependencies so closure behavior is observable without a stored invalidation
  edge family.
- Tests run: `cargo fmt`; `cargo test sg111 --lib`; `cargo test --lib`; `cargo test --test cli_supergraph`; `cargo test --test python_incident_triage_e2e`; `cargo test --test typescript_incident_board_e2e`; `cargo test`. All passed.
- Follow-up: SG-112 is dependency-ready to document JSON schema compatibility and migration rules for persisted `ProgramSupergraph` output and indexed key encodings.

## SG-112 JSON Schema Compatibility

- Added `doc/supergraph-json-compatibility.md` as the durable compatibility contract for persisted `ProgramSupergraph` JSON and linked it from the main supergraph design. It documents schema-version gating, additive vs breaking changes, unknown kind policy, CLI JSON expectations, and consumer migration/testing practices.
- Recorded the persisted fact conventions: stable `node_id`/`edge_id` subject references remain separate from immutable `fact_id`/`payload_hash`; missing fact identity fields in older `program-supergraph.v1` snapshots default empty and should be recomputed during refresh/materialization rather than causing subject churn.
- Documented derived index persistence expectations, including the deterministic
  `SourceSpanIndexKey` JSON map-key encoding with `U+001F`, and clarified
  provenance/evidence/content hashes, confidence vs uncertainty, domain
  knowledge as prose-only, and trace plus derived invalidation views as
  stable-subject recomputation references.
- Tests run: `cargo fmt`; `cargo test serialized_supergraph_preserves_source_span_index_keys_and_defaulted_fact_identity --lib`; `cargo test --test cli_supergraph`; `cargo test --lib`. All passed.
- Follow-up: SG-120 is the next dependency-ready checklist item and can broaden schema completeness coverage beyond the narrow SG-112 serialization convention test.

## SG-120 Schema Completeness Tests

- Added a compact deterministic schema fixture that constructs and JSON roundtrips every current `NodeKind`/`NodeFact` and `EdgeKind`/`EdgeFact` family, asserting kind-to-payload alignment, ownership, evidence/provenance, confidence, span coverage where applicable, stable subject IDs, fact IDs, payload hashes, uncertainty defaults, and family/index membership.
- Treated schema completeness as a fixture and index contract, not a schema redesign: the test covers source-span, artifact/callable ownership, structural containment, symbol definition/use, call summaries, traceability, uncertainty buckets, and invalidation indexes with existing builder refresh behavior.
- Added an SG-120-specific compatibility test for omitted defaulted node and edge envelope fields so SG-112 default-field behavior remains guarded beyond the original source-span key serialization test.
- Tests run: `cargo fmt`; `cargo test sg120 --lib`; `cargo test serialized_supergraph_preserves_source_span_index_keys_and_defaulted_fact_identity --lib`; `cargo test --lib`. All passed.
- Follow-up: SG-121 is dependency-ready for CFG semantic fixtures; keep it focused on executable control-flow behavior rather than expanding the SG-120 schema fixture.

## SG-121 CFG Semantic Tests

- Added executable CFG semantic assertions for shared Python and TypeScript fixtures. The tests compare the exact callable-local `ControlFlow` edge set, including flow kind, SG-053 outcome metadata, branch-arm labels/ordinals, normal return exits, handled exception paths, merge nodes, short-circuit expression-control edges, and basic-block facts through public CFG view helpers.
- Kept switch/match coverage intentionally limited: current parser facts normalize Python `match_statement` and TypeScript `switch_statement` as `Unknown` statements without arm regions, so SG-121 documents and tests the fallback straight-line CFG behavior instead of adding parser or CFG arm lowering.
- Reused SG-054 unreachable fixtures for terminal return, raise/throw, break, and continue diagnostics, and added explicit SG-121 checks for unhandled raise/throw edges to the exceptional callable exit versus handled raise/throw edges to catch handlers.
- Tests run: `cargo fmt`; `cargo test sg121 --lib`; `cargo test sg05 --lib`. Both focused suites passed.
- Follow-up: SG-122 is dependency-ready for post-dominance/control-dependence tests over these CFG conventions. Switch/match arm-level CFG semantics remain blocked until parser facts expose normalized arm regions.

## SG-122 Control-Dependence Tests

- Added shared Python and TypeScript fixtures that assert post-dominance-derived `Controls` facts through the public PDG view instead of relying only on raw edge scans. The assertions cover controlling CFG condition IDs, controlled statement facts, expression facts, call sites, first-class `Definition` facts, data-flow definition nodes, nested conditions, loop bodies, returns, raises/throws, branch merges, and post-merge behavior.
- Minimal semantic fix: `Controls` projection now includes first-class `Definition` nodes at the controlled statement span in addition to the existing statement, expression, call-site, and data-flow definition projections.
- Preserved SG-061 CFG/post-dominance behavior for terminal branches and loops: a condition can control behavior reached only when the alternate outcome terminates or loops, while explicit branch merge nodes and containing branch statements remain uncontrolled by their own condition.
- Tests run: `cargo fmt`; `cargo test sg092 --lib`; `cargo test sg122 --lib`; `cargo test sg06 --lib`; `cargo test`.
- Follow-up: SG-123 is dependency-ready for data-flow and slicing tests over the existing SG-070 through SG-074 conventions.

## SG-123 Data-Flow And Slicing Tests

- Added paired Python and TypeScript SG-123 tests that reuse the existing SG-071 branch/loop fixture, SG-070 expression/call/return fixture, and SG-073 field/index/alias fixture. The tests assert precise value provenance for branch merges, loop-carried values, literals, operators, index operands, call arguments, call expressions, returns, field writes/reads, index writes/reads, and possible alias summaries.
- Kept the tests on existing DFG/PDG public view conventions: `value_forward_slice`, `value_backward_slice`, `return_backward_slice`, `call_backward_slice`, and `branch_backward_slice` verify value influence, controlling conditions, path-condition evidence, and alias diagnostics without adding new semantics or helper APIs.
- Documented current precision conventions in assertions: SG-072 merge/loop-carried edges are possible, SG-073 alias summaries are possible, and field/index write/read summaries can remain probable when fed by probable identifier values while retaining exact SG-073 precision strings.
- Tests run: `cargo fmt`; `cargo test sg123 --lib`; `cargo test sg07 --lib`.
- Follow-up: SG-124 is dependency-ready for interprocedural and SDG tests over SG-080 through SG-085 conventions.

## SG-124 Interprocedural And SDG Tests

- Added paired Python and TypeScript SG-124 public-view tests by extending the existing SG-085 SDG fixture with a second helper artifact/module, receiver-to-self/this flow, possible dynamic local target flow, an unresolved sink, and unresolved-call diagnostics.
- Kept the tests on existing interprocedural and SDG conventions instead of adding new semantics: assertions check `ParameterIn`, receiver `ParameterIn`, `ReturnsTo`, narrowed `ParameterOut`, `ThrowsTo`, `Calls`, value roles/names, exception target metadata, precision strings, uncertainty, entrypoint-to-sink paths, cross-call slices, caller affectedness, and call graph targets through public view APIs.
- Cross-module fixture coverage is synthetic and deterministic so it can assert graph contracts directly without depending on parser import/export normalization; parser-backed end-to-end parity remains appropriate for SG-127.
- Tests run: `cargo fmt`; `cargo test sg124 --lib`; `cargo test sg08 --lib`; `cargo test sg111 --lib`.
- Follow-up: SG-125 is dependency-ready for requirements completeness tests over SG-090 through SG-094 and SG-100 conventions.

## SG-125 Requirements Completeness Tests

- Added paired Python and TypeScript SG-125 tests that compare generated requirements against known fixture behavior instead of only checking source-rule presence. The assertions cover boundary and module-initializer entrypoint requirements, guarded calls/assignments/returns/raises/writes with path conditions, deterministic leaf requirements for constants, assignments, field/index mappings, structural assignment/basic-block facts, generated document rows, and reverse code-to-requirement lookup.
- Kept SG-125 on public requirement/document views where practical: completeness helpers use `RequirementGraphView` traces, locatable code facts, generated rows, `DecomposesTo`, `Conditions`, `Orders`, domain prose dependencies, and reverse lookup. Ordering assertions stay on the SG-093 ordering fixture where the graph records meaningful sequence, while SG-092 remains focused on leaf and guard faithfulness.
- Shared behavior coverage reuses the SG-094 fixture to assert two callers factor through one shared callable requirement, shared guarded wrappers keep two graph-backed conditions, and the original guarded return leaf is not duplicated. Non-behavior/prose/diagnostic facts are asserted not to receive direct generated requirement coverage.
- Tests run: `cargo fmt`; `cargo test sg125 --lib`; `cargo test sg09 --lib`; `cargo test sg100 --lib`. All passed.
- Follow-up: SG-126 is dependency-ready for sync and invalidation tests over SG-101 through SG-105 conventions.

## SG-126 Sync And Invalidation Tests

- Added focused sync/invalidation tests beside the SG-102 through SG-104 fixtures so one compact graph can exercise classification, deterministic edit execution, reanalysis dirty subjects, SG-101 invalidation closure, and SG-105 regenerated document filtering together.
- Covered code fact and artifact invalidation dirtying traced requirements and deterministic regenerated rows/Markdown, plus prose-only domain knowledge dirtying `DependsOnDomainKnowledge` and requirement view rows without dirtying code evidence or artifacts.
- Covered supported Python single-span edit execution, supported TypeScript equivalent multi-span edit execution, rejected structural/ambiguous/unsupported execution, read-only structural generation plans, dirty requirement reporting, and regenerated dirty views from reanalysis subjects.
- Tests run: `cargo fmt`; `cargo test sg126 --lib`; `cargo test sg10 --lib`. Both focused suites passed.
- Follow-up: SG-127 is dependency-ready for supported-language parity tests across requirement, PDG, SDG, and sync coverage.

## SG-127 Supported-Language Parity Tests

- Added a parser-backed public API parity suite for compact equivalent Python and TypeScript fixtures. The shared assertions compare supported semantic capabilities across structural facts, call graph summaries, CFG/PDG controls, DFG values and behavior slices, SDG interprocedural edges, generated requirements, traceability, invalidation closures, regenerated requirement views, and deterministic single-span sync execution.
- Kept parity expectations capability-based instead of exact-count-based because Python and TypeScript syntax wrappers differ; the test asserts equivalent public-view behavior while allowing language-specific node counts and normalization details.
- Documented current switch/match limitations in the parity suite: Python `match` and TypeScript `switch` remain retained as unknown statement facts with fallback CFG behavior, and the tests assert that arm-level CFG metadata is not falsely claimed until parser facts expose normalized arm regions.
- Tests run: `cargo fmt`; `cargo test --test supergraph_language_parity sg127_parser_backed_supported_language_parity_across_public_views`; `cargo test --test supergraph_language_parity`; `cargo test sg12 --lib`; `cargo test`. All passed.
- Follow-up: SG-999 is ready to launch as the final completion gate because every prior checklist task is now checked; it should verify the full documented implementation and complete-suite status rather than add new SG-127 parity semantics.

## SG-999 Final Completion Gate

- Verified the checklist is complete through SG-127 and checked SG-999 after final acceptance passed. The implementation, documentation, public APIs, CLI JSON output, logical views, requirements generation, traceability, invalidation, sync, and regenerated requirement views match `doc/supergraph.md` and `doc/supergraph-json-compatibility.md`.
- Verified public CLI JSON output with `cargo test --test cli_supergraph`; the tests cover deterministic `req-graph supergraph-python` and `req-graph supergraph-typescript` output, parseable `ProgramSupergraph` JSON, schema versioning, indexes, CFG facts, generated requirements, and unsupported-language rejection.
- Verified public graph-backed views and bidirectional requirement behavior with `cargo test --test supergraph_language_parity`; the parser-backed parity suite covers structural, call graph, CFG/PDG, DFG, SDG, requirements, traceability, invalidation closure, regenerated requirement views with `canonical_state: false`, and deterministic single-span sync for Python and TypeScript.
- Verified the required full suite with `cargo test`; all unit tests, CLI tests, Python e2e tests, language parity tests, TypeScript e2e tests, and doc tests passed.
- Residual known limitation: Python `match` and TypeScript `switch` are retained as unknown statement facts with fallback CFG behavior; arm-level CFG metadata is not claimed until parser facts expose normalized arm regions.
- No blockers remain. Generated documents are derived views, not canonical graph state; logical views use filters/indexed traversals over one physical `ProgramSupergraph`; generated requirements are graph-backed, traceable, and synchronized bidirectionally with code through the checked APIs and tests.
