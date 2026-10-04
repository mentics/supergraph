# SuperGraph Implementation Checklist

This document tracks the work required to implement `doc/supergraph.md` completely.
It is designed as an agent-friendly to-do list: every task has a stable ID, a
checkbox, dependencies, expected deliverables, and completion checks.

The current implementation uses direct `analysis::source_graph` lowering into
`ProgramSupergraphBuilder`, semantic enrichment over `ProgramSupergraph`, and
logical views such as `CallGraphView`. The supergraph-only cleanup work that
removed standalone call graph architecture is tracked in
`doc/impl/supergraph/supergraph-only-cleanup-plan.md` and
`doc/impl/supergraph/supergraph-only-cleanup-progress.md`.

## Task Format

- `ID`: Stable identifier for references in chats, commits, and PRs.
- `Status`: `[ ]` not started, `[~]` in progress, `[x]` complete.
- `Depends on`: Task IDs that should be completed first.
- `Deliver`: Concrete files, APIs, behavior, or tests to produce.
- `Done when`: Objective acceptance criteria.

Agents should keep changes scoped to one task or a small dependency chain. When a
task is completed, update only its checkbox and any immediately affected notes.

## Current Baseline

- [x] `SG-000` Shared physical graph exists.
  - Depends on: none
  - Deliver: `ProgramSupergraph`, typed nodes and edges, stable IDs, ownership,
    spans, confidence, evidence, diagnostics, and basic indexes.
  - Done when: `src/supergraph/schema.rs` and `src/supergraph/builder.rs`
    compile and tests verify deterministic sorted graph output.

- [x] `SG-001` Direct source lowering builds the supergraph.
  - Depends on: `SG-000`
  - Deliver: Python and TypeScript `ProjectAst` facts lower through
    `analysis::source_graph` into `ProgramSupergraphBuilder`, then semantic
    enrichment completes the canonical `ProgramSupergraph`.
  - Done when: Python and TypeScript supergraph APIs emit `ProgramSupergraph`
    directly, `CallGraphView` handles call-oriented traversal, there is no
    `src/call_graph` module, no standalone physical `CallGraph`, no whole-graph
    conversion path, and no `call-graph-*` CLI command.

- [x] `SG-002` Initial semantic enrichment pipeline exists.
  - Depends on: `SG-001`
  - Deliver: `enrich_supergraph_with_semantic_flows` runs CFG, DFG, control
    dependence, interprocedural summaries, and requirements generation.
  - Done when: integration tests observe `ControlFlow`, `Controls`, `Defines`,
    `Uses`, `DataFlow`, `ParameterIn`, `ReturnsTo`, `ParameterOut`, `ThrowsTo`,
    requirement, trace, and invalidation facts.

## Phase 1: Schema Completeness

- [x] `SG-010` Add first-class structural node families.
  - Depends on: `SG-000`
  - Deliver: `Statement`, `Expression`, `Condition`, `Symbol`, `Definition`,
    `Use`, `Value`, `BasicBlock`, and `DomainKnowledge` node kinds and payloads.
  - Done when: facts currently collapsed into `ControlFlow` or `DataFlow` can be
    represented as their intended families without losing spans, ownership,
    confidence, or evidence.

- [x] `SG-011` Add missing source and provenance metadata.
  - Depends on: `SG-010`
  - Deliver: parser version, analysis version, content hash, syntax reference,
    original expression text, normalized expression payloads, and source evidence
    fields where needed.
  - Done when: every derived fact can be traced to source spans, analysis
    versions, and parser evidence without embedding large source text or ASTs in
    runtime graph nodes.

- [x] `SG-012` Separate stable subject identity from fact identity.
  - Depends on: `SG-010`
  - Deliver: IDs or fields that distinguish conceptual entities from immutable
    fact versions, aligned with `doc/versioned-graph.md`.
  - Done when: a changed callable body can create new fact payloads without
    changing stable references from callers, requirements, or indexes.

- [x] `SG-013` Represent uncertainty explicitly across all fact families.
  - Depends on: `SG-010`
  - Deliver: exact, probable, possible, external, unresolved, ambiguous, stale,
    and unsupported classifications where applicable.
  - Done when: unresolved calls, ambiguous symbols, unsupported syntax, dynamic
    dispatch, alias uncertainty, and stale domain knowledge are represented as
    graph facts or diagnostics rather than omitted.

- [x] `SG-014` Add graph indexes required by the design.
  - Depends on: `SG-010`
  - Deliver: source-span-to-node, callable-owned nodes, artifact-owned nodes,
    symbol-to-definition, symbol-to-use, requirement-to-code, code-to-requirement,
    and edge-family indexes.
  - Done when: logical views can use indexes instead of scanning the full graph
    for common traversals.

## Phase 2: Semantic Lowering

- [x] `SG-020` Lower parser facts into language-neutral statement facts.
  - Depends on: `SG-010`
  - Deliver: normalized statement nodes with kind, child ordering, ownership,
    spans, and containment edges.
  - Done when: assignments, declarations, expressions, branches, loops, returns,
    raises/throws, function/class declarations, and unknown statements are
    represented consistently for Python and TypeScript.

- [x] `SG-021` Lower parser facts into language-neutral expression facts.
  - Depends on: `SG-020`
  - Deliver: expression nodes for identifiers, literals, calls, fields, indexes,
    assignments, operators, conditional expressions, await/yield where supported,
    and unknown expressions.
  - Done when: call arguments, return values, conditions, assignment RHS values,
    field accesses, index accesses, literals, and operators are available as
    graph facts rather than only text snippets.

- [x] `SG-022` Lower branch, loop, and exception structure.
  - Depends on: `SG-020`, `SG-021`
  - Deliver: explicit facts for branch bodies, else bodies, loop bodies,
    continuation points, try/catch/finally regions, raise/throw behavior, and
    language-specific fallthrough behavior.
  - Done when: CFG construction no longer infers structure only from source-order
    and span containment.

- [x] `SG-023` Lower parameter, return, receiver, and call result values.
  - Depends on: `SG-021`
  - Deliver: value nodes for parameters, receiver/self/this, arguments, return
    values, call result values, exceptional values, and mutable argument state.
  - Done when: interprocedural edges can connect values to values, not call sites
    or return CFG nodes as substitutes.

- [x] `SG-024` Normalize file, module, class, function, and block scopes.
  - Depends on: `SG-010`
  - Deliver: language-neutral scope facts for modules, classes, functions, blocks,
    catch blocks, comprehensions, and language-specific scope variants.
  - Done when: bindings and symbol resolution can target the correct lexical
    scope for nested functions, blocks, classes, comprehensions, and handlers.

## Phase 3: Symbols And Resolution

- [x] `SG-030` Build complete lexical symbol tables.
  - Depends on: `SG-024`
  - Deliver: symbol and binding facts for declarations, imports, parameters,
    assignments, fields, external targets, and unknown targets.
  - Done when: local names, imports/exports, classes, methods, fields, and
    statically visible receivers are represented with exact or uncertain
    resolution facts.

- [x] `SG-031` Resolve uses, symbols, and call sites.
  - Depends on: `SG-030`
  - Deliver: `ResolvesTo` edges from uses, symbols, and call sites to bindings,
    callables, fields, or external targets.
  - Done when: `ResolvesTo` is not limited to binding targets and every use/call
    site has exact, possible, external, unresolved, or ambiguous resolution.

- [x] `SG-032` Derive invalidation dependencies for resolution facts.
  - Depends on: `SG-031`
  - Deliver: invalidation view rules from artifacts, symbol facts,
    import/export facts, parser facts, and existing semantic edges to dependent
    call, data-flow, and requirement facts.
  - Done when: changing a declaration, import, export, or binding can identify
    all derived facts that require recomputation.

## Phase 4: Call Graph View

- [x] `SG-040` Emit calls from call sites to targets.
  - Depends on: `SG-031`
  - Deliver: `Calls` edges from call sites to local callables and external
    targets, plus targetless `Calls` edges for unresolved and dynamic
    non-callable calls.
  - Done when: the call graph view can answer call-site-to-target and
    caller-to-callee queries from current supergraph call facts.

- [x] `SG-041` Build call traversal indexes.
  - Depends on: `SG-040`
  - Deliver: all-call caller and call-site indexes, plus concrete-target indexes
    for callable and external-target traversal.
  - Done when: `CallGraphView` queries remain efficient without materializing a
    second graph model.

- [x] `SG-042` Capture TypeScript module-initializer calls.
  - Depends on: `SG-040`
  - Deliver: TypeScript top-level call facts equivalent to Python file-level
    module-initializer calls.
  - Done when: TypeScript files with top-level calls produce call sites and call
    edges owned by module initializer callables.

## Phase 5: Control Flow Graph

- [x] `SG-050` Replace source-order CFG with structured recursive CFG lowering.
  - Depends on: `SG-022`
  - Deliver: CFG construction over statement lists and expression control
    regions.
  - Done when: sequences, branches, loops, returns, raises/throws, break,
    continue, try/catch/finally, and short-circuit expressions are modeled by
    structured lowering rather than global source-order sorting.

- [x] `SG-051` Add normal and exceptional callable exits.
  - Depends on: `SG-050`
  - Deliver: separate normal exit and exceptional exit nodes for languages with
    exceptions.
  - Done when: returns flow to normal exit, unhandled raises/throws flow to
    exceptional exit, and handled exceptions flow to handlers.

- [x] `SG-052` Add merge and optional basic-block nodes.
  - Depends on: `SG-050`
  - Deliver: merge nodes where branch paths rejoin and optional compact
    `BasicBlock` nodes for straight-line regions.
  - Done when: if/else, switch/match, loops, and exception regions have explicit
    rejoin points.

- [x] `SG-053` Add branch outcome metadata.
  - Depends on: `SG-050`
  - Deliver: true, false, arm, fallthrough, exception, finally, break, continue,
    and loop-back metadata on `ControlFlow` edges.
  - Done when: a traversal can distinguish why each control-flow successor is
    reachable.

- [x] `SG-054` Detect unreachable statements.
  - Depends on: `SG-050`
  - Deliver: diagnostics for unreachable statements and facts that explain which
    terminal statement prevents reachability.
  - Done when: statements after unconditional return/raise/break/continue in the
    same region are reported and are not connected by normal sequential flow.

## Phase 6: Control Dependence

- [x] `SG-060` Implement dominator and post-dominator analysis.
  - Depends on: `SG-050`, `SG-051`
  - Deliver: reusable callable-local dominator/post-dominator computation over
    CFG nodes.
  - Done when: tests cover branches, loops, merges, terminal paths, and
    exceptional paths.

- [x] `SG-061` Derive `Controls` edges from post-dominance.
  - Depends on: `SG-060`
  - Deliver: condition-to-controlled-fact edges based on branch-controlled
    execution rather than syntactic containment.
  - Done when: a node is controlled by a condition only when choosing one branch
    can make the node execute and another can avoid it.

- [x] `SG-062` Generate path condition summaries.
  - Depends on: `SG-061`
  - Deliver: summaries attached to controlled behavior for requirement
    generation and requirements-to-code planning.
  - Done when: requirement generation can state the conditions under which
    assignments, calls, returns, raises, and side effects occur.

## Phase 7: Intraprocedural Data Flow

- [x] `SG-070` Introduce value-instance data flow.
  - Depends on: `SG-023`, `SG-050`
  - Deliver: value facts for parameters, assignments, literals, operators, calls,
    fields, indexes, returns, merges, and loop-carried values.
  - Done when: `DataFlow` edges connect value sources to value destinations, not
    only same-name definitions and uses.

- [x] `SG-071` Compute CFG-aware reaching definitions.
  - Depends on: `SG-070`
  - Deliver: forward data-flow analysis over CFG paths.
  - Done when: definitions reaching a use account for branches, loops, terminal
    paths, and merges.

- [x] `SG-072` Add merge and loop-carried values.
  - Depends on: `SG-071`
  - Deliver: value merge facts for branch joins and loop-carried variables.
  - Done when: a use after a branch can trace to all possible reaching
    definitions with explicit uncertainty or merge representation.

- [x] `SG-073` Model fields, properties, indexes, and aliases.
  - Depends on: `SG-071`
  - Deliver: field value definitions, field reads, index value summaries, alias
    diagnostics, and uncertain flow summaries.
  - Done when: reads and writes through object fields or indexes participate in
    forward and backward slices.

- [x] `SG-074` Support forward and backward slices.
  - Depends on: `SG-070`, `SG-061`
  - Deliver: traversal helpers or views for value-forward and behavior-backward
    slicing.
  - Done when: callers can ask where a value flows and what values/conditions
    influence a return, write, external call, branch, or emitted event.

## Phase 8: Interprocedural Data Flow And SDG

- [x] `SG-080` Connect actual argument values to formal parameter values.
  - Depends on: `SG-023`, `SG-070`, `SG-040`
  - Deliver: `ParameterIn` edges from argument value nodes to callee parameter
    value nodes.
  - Done when: each resolved or possible call connects actual values to formals
    with exact or uncertain confidence.

- [x] `SG-081` Connect receiver values to self/this parameters.
  - Depends on: `SG-080`
  - Deliver: receiver-to-formal edges for methods and constructors.
  - Done when: method calls expose object state flow across call boundaries.

- [x] `SG-082` Connect return summaries to call result values.
  - Depends on: `SG-080`
  - Deliver: `ReturnsTo` edges from callee return value summaries to caller call
    result value nodes.
  - Done when: call result values can be sliced backward into callee returns and
    forward into caller uses.

- [x] `SG-083` Model parameter-out mutation summaries.
  - Depends on: `SG-073`, `SG-080`
  - Deliver: `ParameterOut` edges only for known or possible receiver/argument
    mutations, with alias/uncertainty metadata.
  - Done when: parameter-out facts no longer overstate mutation behavior for
    every parameter by default.

- [x] `SG-084` Model exceptional interprocedural flow.
  - Depends on: `SG-051`, `SG-080`
  - Deliver: `ThrowsTo` edges from raised/thrown value summaries to catch handlers,
    caller exceptional paths, or exceptional exits.
  - Done when: exception type or uncertainty is represented and handled exceptions
    do not flow directly to caller exceptional exit.

- [x] `SG-085` Complete SDG traversal view.
  - Depends on: `SG-080`, `SG-081`, `SG-082`, `SG-083`, `SG-084`
  - Deliver: SDG view over PDG plus call and interprocedural value edges.
  - Done when: cross-call slicing can find entrypoints, sinks, callers affected by
    callee changes, and requirement slices across function/module boundaries.

## Phase 9: Requirements Graph

- [x] `SG-090` Generate boundary and entrypoint requirements from graph evidence.
  - Depends on: `SG-085`
  - Deliver: high-level requirements for modules, routes, exported functions,
    components, tests, configs, and other boundary artifacts.
  - Done when: every entrypoint candidate has a traced requirement backed by
    deterministic graph evidence.

- [x] `SG-091` Generate condition and guarded behavior requirements.
  - Depends on: `SG-062`
  - Deliver: condition requirements and child requirements connected by
    `Conditions`.
  - Done when: guarded assignments, calls, returns, raises, writes, and side
    effects include path conditions in generated requirement text.

- [x] `SG-092` Generate leaf behavior requirements.
  - Depends on: `SG-070`, `SG-085`
  - Deliver: requirements for assignments, constants, returns, raises, writes,
    field mappings, validations, external calls, emitted events, and structural
    facts.
  - Done when: deterministic implementation-level behavior has code-backed leaf
    requirements.

- [x] `SG-093` Generate ordering and decomposition relationships.
  - Depends on: `SG-090`, `SG-091`, `SG-092`
  - Deliver: `Orders` and `DecomposesTo` edges where sequence and hierarchy are
    semantically meaningful.
  - Done when: requirements can be traversed from boundary behavior to exact
    implementation-level leaves without relying on document ordering.

- [x] `SG-094` Factor shared behavior into reusable requirements.
  - Depends on: `SG-093`
  - Deliver: requirement nodes for shared behavior used by multiple callers,
    entrypoints, or conditions.
  - Done when: common code-side behavior does not produce duplicated, inconsistent
    requirements.

- [x] `SG-095` Integrate domain knowledge for human-readable text only.
  - Depends on: `SG-092`
  - Deliver: `DomainKnowledge` facts and `DependsOnDomainKnowledge` style
    traceability for generated prose.
  - Done when: domain knowledge improves wording without creating deterministic
    code facts.

## Phase 10: Traceability, Invalidation, And Sync

- [x] `SG-100` Make every generated requirement trace to exact code-backed facts.
  - Depends on: `SG-090`, `SG-091`, `SG-092`
  - Deliver: bidirectional `TracesTo` coverage for requirement-to-code and
    code-to-requirement traversal.
  - Done when: every generated requirement has enough trace evidence to identify
    source spans or structural code facts.

- [x] `SG-101` Complete invalidation graph.
  - Depends on: `SG-032`, `SG-100`
  - Deliver: a derived invalidation view from artifacts and facts to parser
    outputs, semantic facts, analyses, requirements, generated documents, and
    domain knowledge dependencies.
  - Done when: a changed artifact or graph fact can mark all affected derived
    facts dirty without recomputing unrelated facts.

- [x] `SG-102` Implement requirements-to-code edit classification.
  - Depends on: `SG-100`, `SG-074`, `SG-085`
  - Deliver: deterministic single-span, deterministic multi-span, structural,
    ambiguous, and unsupported classifications using graph slices.
  - Done when: classifications account for spans, expressions, symbols, bindings,
    path conditions, affected slices, and formatting ownership boundaries.

- [x] `SG-103` Execute deterministic code edits.
  - Depends on: `SG-102`
  - Deliver: edit application for safe single-span and equivalent multi-span
    requirement changes.
  - Done when: supported requirement edits modify source code, preserve formatting
    boundaries, and trigger invalidation/reanalysis.

- [x] `SG-104` Plan structural code generation edits.
  - Depends on: `SG-102`
  - Deliver: structured plans for edits requiring new code, moved code, new
    declarations, or changed control/data flow.
  - Done when: unsupported direct edits return actionable graph-backed plans with
    affected facts and ambiguity points.

- [x] `SG-105` Regenerate human-readable requirement views.
  - Depends on: `SG-101`
  - Deliver: generated Markdown/doc/table views over the requirements graph.
  - Done when: generated documents are not canonical state and can be regenerated
    from graph facts after invalidation.

## Phase 11: Public APIs And CLI

- [x] `SG-110` Add public supergraph CLI commands.
  - Depends on: `SG-002`
  - Deliver: commands for Python and TypeScript supergraph JSON output.
  - Done when: users can run the CLI to emit `ProgramSupergraph` directly.

- [x] `SG-111` Add public view APIs.
  - Depends on: `SG-014`, `SG-085`, `SG-100`
  - Deliver: stable APIs for structural, call graph, CFG, DFG, PDG, SDG,
    requirements, traceability, and invalidation views.
  - Done when: downstream callers do not need to scan raw nodes and edges for
    common graph questions.

- [x] `SG-112` Document JSON schema versioning rules.
  - Depends on: `SG-010`, `SG-012`
  - Deliver: schema versioning and evolution notes for persisted supergraph
    facts.
  - Done when: serialized graph outputs can evolve without silently breaking
    consumers.

## Phase 12: Tests And Verification

- [x] `SG-120` Add schema completeness tests.
  - Depends on: `SG-010`
  - Deliver: tests for all intended node and edge families, ownership, evidence,
    confidence, spans, stable IDs, and indexes.
  - Done when: missing or mismatched schema families are caught by tests.

- [x] `SG-121` Add CFG semantic tests.
  - Depends on: `SG-050`, `SG-051`, `SG-052`, `SG-053`, `SG-054`
  - Deliver: Python and TypeScript fixtures for branches, loops, merges,
    unreachable statements, break/continue, return, raise/throw, try/catch/finally,
    switch/match where applicable, and short-circuit expressions.
  - Done when: CFG tests assert exact expected control-flow edges and metadata.

- [x] `SG-122` Add control-dependence tests.
  - Depends on: `SG-060`, `SG-061`
  - Deliver: fixtures proving post-dominance-based `Controls` behavior.
  - Done when: tests distinguish true dependence from mere syntactic containment.

- [x] `SG-123` Add data-flow and slicing tests.
  - Depends on: `SG-070`, `SG-071`, `SG-072`, `SG-073`, `SG-074`
  - Deliver: fixtures for branches, loops, merges, fields, aliases, literals,
    operators, calls, returns, and forward/backward slices.
  - Done when: tests assert precise value provenance and influence relationships.

- [x] `SG-124` Add interprocedural and SDG tests.
  - Depends on: `SG-080`, `SG-081`, `SG-082`, `SG-083`, `SG-084`, `SG-085`
  - Deliver: cross-function and cross-module fixtures for parameters, receivers,
    returns, mutations, exceptions, external calls, unresolved calls, and possible
    dynamic targets.
  - Done when: cross-call slices produce expected paths and uncertainty.

- [x] `SG-125` Add requirements completeness tests.
  - Depends on: `SG-090`, `SG-091`, `SG-092`, `SG-093`, `SG-094`, `SG-100`
  - Deliver: tests that compare generated requirement coverage against known
    fixture behavior.
  - Done when: tests verify behavioral faithfulness, not just presence of
    requirement kinds.

- [x] `SG-126` Add sync and invalidation tests.
  - Depends on: `SG-101`, `SG-102`, `SG-103`, `SG-104`, `SG-105`
  - Deliver: tests for changed code invalidating requirements and supported
    requirement edits changing code.
  - Done when: graph slices are recomputed, affected requirements are marked
    dirty, and generated views update deterministically.

- [x] `SG-127` Add parity tests for supported languages.
  - Depends on: `SG-121`, `SG-123`, `SG-124`, `SG-125`
  - Deliver: Python and TypeScript coverage for equivalent semantic features.
  - Done when: TypeScript has requirement, PDG, SDG, and sync coverage comparable
    to Python.

## Final Completion Gate

- [x] `SG-999` SuperGraph is implemented 100%.
  - Depends on: all tasks above
  - Deliver: implementation, documentation, APIs, CLI, and tests matching
    `doc/supergraph.md`.
  - Done when:
    - Every task in this document is checked.
    - `cargo test` passes.
    - Public CLI can emit supergraph JSON.
    - Logical views are explicit filters or indexed traversals over one physical
      graph.
    - Generated requirements are comprehensive, graph-backed, traceable, and
      bidirectionally synchronized with code.
    - No generated document is treated as canonical graph state.
