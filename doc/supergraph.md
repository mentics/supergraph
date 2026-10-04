# Program Supergraph Design

This document describes the graph architecture for the analysis engine. The
system uses one physical typed program graph, called the program supergraph, and
exposes call graph, control flow graph, data flow graph, program dependence
graph, requirements graph, traceability graph, and invalidation graph as logical
views over that shared graph.

The goal is comprehensive bidirectionality. The graph must support generating
requirements from code and updating code from requirements, from high-level
boundary behavior down to exact implementation-level behavior.

## Core Decision

Use one physical graph with strongly typed nodes and edges:
`ProgramSupergraph`.

The graph must not be an undifferentiated collection of relationships. Every
node and edge must have a type, stable identity, source provenance, confidence,
and ownership information. Logical graphs are produced by filtering node and
edge families through explicit views such as `CallGraphView`.

For example:

- The call graph view follows callable, call site, external target, and `Calls`
  edges.
- The control flow graph view follows control nodes and `ControlFlow` edges.
- The data flow graph view follows value locations and `DataFlow`, `Defines`,
  `Uses`, `ParameterIn`, `ParameterOut`, and `ReturnsTo` edges.
- The program dependence graph view follows data dependence and control
  dependence edges.
- The requirements graph view follows requirement nodes and `DecomposesTo`,
  `Conditions`, `Orders`, and `TracesTo` edges.

This gives the engine one shared representation for cross-cutting questions such
as:

> Which boundary requirement decomposes into this condition, which controls this
> assignment, whose value flows into this return, which is exposed through this
> entrypoint?

## Architecture Overview

The analysis pipeline builds `ProgramSupergraph` directly:

1. Discover supported source files.
2. Parse each file into `ProjectAst`.
3. Lower language-specific `ProjectAst` facts directly into
   `ProgramSupergraphBuilder` through the source-graph lowerer and language
   adapters.
4. Resolve symbols, scopes, bindings, call sites, external targets, unresolved
   calls, and dynamic calls as supergraph nodes and edges.
5. Run semantic enrichment over the resulting `ProgramSupergraph` to add CFG,
   control-dependence, data-flow, requirements, traceability, and invalidation
   facts.
6. Query the graph through logical views such as `CallGraphView`,
   `ControlFlowGraphView`, `DataFlowGraphView`, and
   `ProgramDependenceGraphView`.

There is no standalone physical `CallGraph` model in the architecture. A call
graph is a logical view only: `CallGraphView` filters and traverses
`ProgramSupergraph` call facts, optionally using `CallGraphFilter` and canonical
call indexes for efficient access.

The builder emits typed graph facts directly:

- `GraphNode { node_id, kind, owner, span, confidence, evidence, ... }`
- `GraphEdge { edge_id, kind, source_id, target_id, confidence, evidence, ... }`
- analysis-specific payloads for facts that need structured data

Call-related lowering produces `Callable`, `CallSite`, `ExternalTarget`,
`Calls`, `ParameterIn`, `ParameterOut`, and `ReturnsTo` facts inside
`ProgramSupergraph`, not a separate call graph snapshot.

## Physical Graph Shape

The physical graph should be compact and traversal-oriented. Large source text,
AST fragments, and generated requirement prose should live in persistent stores
or content-addressed caches, not directly inside runtime graph nodes.
Persisted JSON snapshots should follow the schema versioning and compatibility
rules in `doc/supergraph-json-compatibility.md`.

Typical node families:

- `Artifact`: source file, test file, schema, config, generated output, or other
  `dev artifact`.
- `Scope`: module, class, function, block, catch block, comprehension, or
  language-specific scope normalized into shared categories.
- `Callable`: function, method, constructor, closure, lambda, module initializer,
  getter, setter, or other executable unit.
- `Statement`: normalized executable statement.
- `Expression`: normalized expression that participates in control, data, call,
  or requirement evidence.
- `Condition`: branch, loop, guard, match arm, catch filter, short-circuit, or
  conditional expression predicate.
- `BasicBlock`: optional compact CFG node grouping straight-line statements.
- `Symbol`: resolved or unresolved name in a lexical scope.
- `Binding`: association between a name and a declaration, import, parameter,
  assignment, field, external target, or unknown target.
- `Definition`: a value-producing program point.
- `Use`: a value-consuming program point.
- `Value`: literal, computed expression, object value, field value, call result,
  parameter value, return value, or merged value.
- `CallSite`: syntactic call expression.
- `ExternalTarget`: unresolved or external callable-like target.
- `Requirement`: generated or user-authored requirement node.
- `DomainKnowledge`: stored domain knowledge record used to improve human
  readable outputs without changing deterministic analysis.
- `Diagnostic`: unresolved, ambiguous, uncertain, unsupported, or stale fact.

Typical edge families:

- `Contains`: artifact to scope, scope to callable, callable to statement,
  statement to expression, requirement to child requirement view ownership.
- `Binds`: scope or symbol to binding.
- `ResolvesTo`: use, call site, or symbol to binding, callable, field, or
  external target.
- `Calls`: call site to a concrete callable or external target when one is
  known; unresolved and dynamic non-callable calls remain targetless and keep
  their target text in the `Calls` payload.
- `ControlFlow`: executable control predecessor to executable successor.
- `Controls`: condition to behavior that is control-dependent on that condition.
- `Defines`: definition to symbol, field, or value location.
- `Uses`: expression, condition, call argument, return, or assignment to symbol,
  field, or value location.
- `DataFlow`: value source to value destination.
- `ParameterIn`: call argument value to callee parameter value.
- `ParameterOut`: callee output, mutation, or exceptional output to call site.
- `ReturnsTo`: return value to call result value.
- `ThrowsTo`: raise or throw value to catch handler, caller, or exceptional exit.
- `DecomposesTo`: higher-level requirement to lower-level requirement.
- `Conditions`: requirement to the condition requirement or code condition that
  guards it.
- `Orders`: requirement to another requirement when sequencing matters.
- `TracesTo`: requirement to code-backed facts and code-backed facts back to
  requirements.
- `DependsOnDomainKnowledge`: generated human-readable output to domain
  knowledge records.

## Logical Views

Logical views are not separate conceptual systems. They are filtered traversals
or indexes over the same physical graph.

### Structural View

The structural view follows `Contains`, `Binds`, and `ResolvesTo` edges.

It answers:

- Which artifacts, scopes, callables, statements, and expressions exist?
- Which symbols are declared in a scope?
- Which use resolves to which binding?
- Which source span owns this graph fact?

This view is the base for most later analyses.

### Call Graph View

`CallGraphView` follows `Callable`, `CallSite`, `ExternalTarget`, `Calls`,
`ParameterIn`, `ParameterOut`, `ReturnsTo`, and related resolution edges.
`CallGraphFilter` selects local, external, unresolved, or all calls.

It answers:

- Which callables can call which other callables?
- Which callables are entrypoint candidates?
- Which external APIs, libraries, framework hooks, or unresolved targets are on
  a path?
- How do argument values enter a callee?
- How do return values, mutations, and exceptions leave a callee?

The view is backed by `ProgramSupergraph` indexes. Caller and call-site indexes
cover all `Calls` edges, including targetless unresolved and dynamic calls.
Concrete-target indexes cover only calls whose edge target is a callable or
external target.

### Control Flow Graph View

The CFG view follows `ControlFlow` edges within each callable.

It answers:

- What can execute after this program point?
- Which paths reach this return, raise, assignment, call, or side effect?
- Where do branches merge?
- Which loops can repeat?
- Which statements are unreachable?
- Which exceptional paths leave a callable?

CFG nodes should be language-neutral control nodes backed by source spans. The
graph can use statement-level nodes for precision and optionally create
`BasicBlock` nodes as a compact index.

### Control Dependence View

The control dependence view follows `Controls` edges derived from the CFG.

It answers:

- Which condition controls this assignment, return, raise, call, or side effect?
- Which requirements are guarded by which path conditions?
- Which branch outcome must change if a requirement changes?

Control dependence should be derived from the CFG using post-dominance
information. A node is control-dependent on a branch when the branch determines
whether that node executes.

### Data Flow Graph View

The DFG view follows `Defines`, `Uses`, `DataFlow`, `ParameterIn`,
`ParameterOut`, `ReturnsTo`, and `ThrowsTo` edges.

It answers:

- Where can this value come from?
- Where can this value flow?
- Which definitions can reach this use?
- Which inputs influence this return, write, external call, branch, or emitted
  event?
- Which fields, object properties, or aliases may carry this value?

The DFG should use the CFG to compute path-sensitive or path-aware data-flow
facts. It is not a replacement for the CFG.

### Program Dependence Graph View

The program dependence graph, or PDG, combines data dependence and control
dependence within a callable.

It answers:

- Which values and conditions explain this behavior?
- Which behavior is affected by changing this definition, condition, or constant?
- Which statements belong to the slice for this symbol or value?

For bidirectional requirements, the PDG is the main code-side explanation layer.

### System Dependence Graph View

The system dependence graph, or SDG, extends the PDG across call boundaries using
`Calls` edges and parameter/return edges.

It answers:

- Which entrypoints can pass this value to this sink?
- Which callers are affected by changing this callee requirement?
- Which requirement slice crosses function, method, module, or package
  boundaries?

The SDG is the right view for cross-call slicing and impact analysis.

### Requirements Graph View

The requirements graph view follows `Requirement`, `DecomposesTo`,
`Conditions`, `Orders`, and `TracesTo` edges.

It answers:

- Which high-level requirements decompose into this implementation-level
  behavior?
- Which leaf requirements are backed by this code span?
- Which requirement changed when this code changed?
- Which code facts must change when this requirement changes?

Generated Markdown documents and tables are views over this graph, not the
canonical representation.

## Generation Pipeline

The comprehensive pipeline generates the supergraph in layers. These layers are
ordered by dependency, but they all write into the same physical graph.

### 1. Discover Artifacts

Discover source files, test files, schemas, config files, generated outputs, and
other `dev artifacts`.

Emit:

- `Artifact` nodes.
- `Contains` edges from repository root or package roots to artifacts.
- Ownership, source-span, and provenance metadata used by the invalidation view
  to derive parser outputs and derived facts affected by artifact changes.

### 2. Parse To Language ASTs

Parse each supported source artifact with the appropriate language parser.

The AST is not the runtime graph. It is parser output used to produce normalized
semantic facts.

Emit or persist:

- content hash
- parse diagnostics
- exact source spans
- syntax references for later evidence lookup

### 3. Lower ProjectAst To Source Graph Facts

Lower language-specific `ProjectAst` records directly into language-neutral
supergraph facts for artifacts, scopes, callables, statements, expressions,
conditions, declarations, bindings, uses, definitions, calls, returns, raises,
and field accesses. This lowering writes through `ProgramSupergraphBuilder`.

This stage should preserve enough structure to build CFG and DFG later. The
`ProjectAst` model captures source facts, and the supergraph enriches them with
statement and expression facts:

- statement kind
- expression kind
- child expression ordering
- branch bodies and else bodies
- loop body and loop continuation behavior
- try, catch, finally, raise, throw, await, yield, and return behavior
- assignment target shape
- call argument expressions, not only counts and names
- field and index access paths
- literal values and operators

Emit:

- `Scope`, `Callable`, `Statement`, `Expression`, `Condition`, `Symbol`,
  `Binding`, `Definition`, `Use`, `CallSite`, and `ExternalTarget` nodes.
- `Contains`, `Binds`, `Defines`, `Uses`, and preliminary `ResolvesTo` edges.

### 4. Build Symbols And Resolution Facts

Build lexical scopes and symbol tables for the whole project or invalidated
slice.

Resolve:

- local names
- imports and exports
- class and method names
- fields and properties when statically visible
- call receivers when statically visible
- external targets
- ambiguous or unresolved references

Emit:

- exact, probable, possible, external, and unresolved `ResolvesTo` edges.
- diagnostics for ambiguity, unsupported language features, and unresolved
  symbols.
- Resolution, ownership, and provenance facts that let the invalidation view
  derive dependent call, data-flow, and requirement facts.

### 5. Build Call Facts

Use supergraph-native call resolution to connect call sites to local callables,
external targets, possible dynamic targets, or unresolved targets.

Emit:

- `Calls` edges from call sites to concrete callable or external targets.
- targetless `Calls` edges for unresolved calls and dynamic non-callable calls,
  with the unresolved target text retained in the payload.
- caller and call-site indexes for all calls.
- concrete-target indexes for callable and external-target traversal.
- `ParameterIn` edges from call argument values to callee parameter values when
  the call target is known or possible.
- `ReturnsTo` edges from callee return summary values to call result values.
- `ParameterOut` edges for mutations of receivers, mutable arguments, or known
  out-parameters when modeled.
- `ThrowsTo` edges for known exceptional outputs.

### 6. Build Control Flow Graphs

Build a CFG for each callable, including module initializer callables.

Each callable should have:

- an entry node
- a normal exit node
- an exceptional exit node when the language supports exceptions
- statement or basic block nodes
- condition nodes
- merge nodes where paths rejoin
- loop back edges
- break, continue, return, raise, throw, yield, and await behavior

Generation should be a structured recursive lowering over statement lists:

- A sequence connects the exit of each statement to the entry of the next.
- An `if` or conditional expression creates a condition node with true and false
  successors, then merges paths that continue.
- A `match` or `switch` creates one condition/dispatch region with one successor
  per arm and explicit fallthrough behavior where the language allows it.
- A loop connects the loop condition to body and exit paths, with body exits
  flowing back to the loop condition or update step.
- `break` connects to the nearest loop or switch exit.
- `continue` connects to the nearest loop continuation point.
- `return` connects to callable normal exit.
- `raise` or `throw` connects to the nearest handler or exceptional exit.
- `try` connects protected statements to catch handlers and finally blocks using
  both normal and exceptional edges.
- Boolean short-circuit operators create control nodes because the right-hand
  expression may or may not execute.

Emit:

- `ControlFlow` edges.
- reachability diagnostics for unreachable statements.
- loop membership metadata.
- branch outcome metadata for true, false, arm, exception, finally, break, and
  continue edges.

### 7. Derive Control Dependence

After building CFGs, compute dominators and post-dominators per callable.

Use post-dominance to derive `Controls` edges:

- A condition controls a node when choosing one branch can cause that node to
  execute while another branch can avoid it.
- Loop conditions control loop body behavior.
- Exception handlers are controlled by the protected region and matching
  exception path.
- Short-circuit expressions control evaluation of later operands.

Emit:

- `Controls` edges from condition nodes to controlled statements, expressions,
  definitions, calls, returns, raises, and side effects.
- path condition summaries for requirement generation.

### 8. Build Intraprocedural Data Flow

Build data-flow facts inside each callable using definitions, uses, and CFG
reachability.

A comprehensive design should model value instances rather than only symbol
names. For example, `x` before an assignment and `x` after an assignment are
different value facts.

Use an SSA-like internal representation where helpful:

- parameters create initial value definitions.
- assignments create new value definitions.
- branches that rejoin create merge values.
- loop-carried values create loop merge values.
- field writes create field value definitions.
- field reads use the latest known field value or an uncertain field summary.
- literals, operators, and calls create expression values.

Emit:

- `DataFlow` edges from value sources to value destinations.
- `Defines` edges from definitions to symbols, fields, or value locations.
- `Uses` edges from expressions, conditions, calls, returns, and side effects to
  the values they consume.
- merge nodes or metadata for values with multiple reaching definitions.
- diagnostics for unsupported aliasing, dynamic property access, reflection,
  `eval`, monkey patching, or other uncertain flows.

The DFG should support both forward and backward slices:

- Forward slice: where can this value flow?
- Backward slice: what values and conditions influence this value or behavior?

### 9. Build Interprocedural Data Flow

Use call facts from `ProgramSupergraph` to connect caller and callee data flow.

For each call site:

- Connect argument value nodes to callee parameter value nodes with
  `ParameterIn`.
- Connect receiver value nodes to callee receiver/self/this parameter nodes.
- Connect callee return value summaries to call result nodes with `ReturnsTo`.
- Connect known mutations of receiver or argument state back to caller-visible
  values with `ParameterOut`.
- Connect raised or thrown values to catch handlers or caller exceptional paths
  with `ThrowsTo`.

When call resolution is uncertain, emit possible edges with uncertainty metadata
instead of hiding the relationship.

### 10. Build PDG And SDG Views

The PDG and SDG do not need to duplicate nodes. They are traversal views:

- PDG: `DataFlow` plus `Controls` within callable boundaries.
- SDG: PDG plus `Calls`, `ParameterIn`, `ReturnsTo`, `ParameterOut`, and
  `ThrowsTo` across callable boundaries.

Create indexes that make these traversals fast:

- outgoing and incoming edge lists by edge family
- callable-owned node sets
- artifact-owned node sets
- source-span-to-node indexes
- symbol-to-definition and symbol-to-use indexes
- requirement-to-code and code-to-requirement indexes

### 11. Generate Requirements

Generate requirement nodes from the SDG and structural facts.

Boundary and entrypoint requirements are high-level views. They should decompose
into lower-level requirements until every deterministic behavior and required
structure is represented.

Generation rules:

- A callable can produce functional requirements describing its observable
  behavior, parameters, returns, raised errors, calls, and side effects.
- A condition can produce a condition requirement.
- Behavior controlled by a condition can produce child requirements connected by
  `Conditions`.
- Sequential behavior can produce ordered requirements connected by `Orders`
  when sequence is meaningful.
- Shared behavior can be factored into reusable requirement nodes connected by
  `DecomposesTo`.
- Assignments, constants, returns, raises, writes, field mappings, validation
  checks, external calls, and emitted events can produce leaf requirements.
- Structural facts can produce structural requirements for modules, callables,
  parameters, fields, schemas, routes, tests, and config.

Each generated requirement must trace to exact code-backed facts with
`TracesTo` edges and evidence spans.

### 12. Support Requirements-To-Code Sync

When a requirement changes, traverse from the requirement node to code-backed
facts.

The sync engine should classify the edit:

- deterministic single-span edit
- deterministic multi-span equivalent edit
- structural edit requiring code generation
- ambiguous edit requiring user or agent assistance
- unsupported edit requiring a diagnostic

The supergraph should preserve enough structure for direct edits:

- source spans
- original expression text
- normalized expression facts
- symbol and binding IDs
- path conditions
- affected slices
- formatting ownership boundaries

When code or requirements change, query the derived invalidation view to mark
affected subjects dirty, recompute graph slices, and regenerate human-readable
views. The view derives its closure from ownership, spans, provenance,
`TracesTo`, `DependsOnDomainKnowledge`, and existing semantic edges rather than
persisting a separate invalidation edge family.

## Why CFG And DFG Are Both Required

CFG and DFG are not alternatives and neither is a superset of the other.

The CFG explains execution possibility and ordering:

```text
condition -> true branch -> merge -> return
condition -> false branch -> merge -> return
```

The DFG explains value movement:

```text
literal 25 -> definition of batch_size -> use of batch_size
```

The requirement:

> If `len(items) > 1000`, the batch size must be `25`.

requires both:

- `Controls`: the assignment is controlled by the condition
  `len(items) > 1000`.
- `DataFlow`: the literal value `25` flows into the definition of `batch_size`.

That combined explanation lives in the PDG/SDG view over the supergraph.

## Implementation Shape

The implementation centers on the shared `ProgramSupergraph` schema and direct
source-graph lowering path.

Recommended modules:

- `supergraph::schema`: shared node, edge, ID, source span, confidence, evidence,
  and diagnostic types.
- `supergraph::builder`: `ProgramSupergraphBuilder`, stable ID helpers,
  ownership tracking, fact refresh, deterministic sorting, and index
  construction.
- `supergraph::views`: typed traversal filters and view types such as
  `CallGraphView` and `CallGraphFilter` for call graph, CFG, DFG, PDG, SDG,
  requirements, traceability, and invalidation traversals.
- `analysis::source_graph`: direct language-neutral lowering from `ProjectAst`
  facts into `ProgramSupergraphBuilder`.
- `analysis::symbols`: scopes, bindings, imports, exports, and resolution.
- `analysis::calls`: supergraph-native call fact emission from call-site
  resolution facts.
- `analysis::cfg`: callable-local CFG construction.
- `analysis::control_dependence`: dominator/post-dominator and `Controls` edge
  derivation.
- `analysis::data_flow`: intraprocedural data-flow analysis.
- `analysis::interprocedural`: call-boundary data-flow summaries and SDG edges.
- `analysis::requirements`: requirement generation and trace linking.
- `sync`: requirements-to-code and code-to-requirements invalidation planning.

The public analysis entry points return either parser output (`ProjectAst`) or
the canonical graph snapshot (`ProgramSupergraph`). Call-oriented consumers use
`ProgramSupergraph::call_graph_view()` instead of asking the analysis pipeline to
materialize another graph model.

## Design Rules

- Prefer exact deterministic facts, but represent uncertainty explicitly.
- Never hide unresolved calls, ambiguous symbols, or possible dynamic behavior.
- Keep parser ASTs out of runtime graph nodes.
- Keep stable IDs separate from runtime graph indexes.
- Make every derived fact traceable to source spans and analysis versions.
- Make every logical graph view an explicit edge filter.
- Build requirements from graph evidence, not from LLM guesses.
- Use domain knowledge only to improve human-readable language, not to create
  deterministic code facts.
- Treat generated documents as views over the requirements graph, not as the
  canonical model.
