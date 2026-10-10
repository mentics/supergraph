# AST To Supergraph Lowering

This document describes how the analysis pipeline turns parsed source code into
the canonical `ProgramSupergraph`. Contributor work should build and enrich this
one physical graph directly; call graph behavior is exposed later through
`CallGraphView` over the same facts.

The design intentionally separates language-specific AST traversal from the
language-neutral supergraph schema. Each language adapter reads `ProjectAst`
records, normalizes source facts, and writes typed nodes and edges through
`ProgramSupergraphBuilder`.

## Goals

- Lower local source artifacts into `ProgramSupergraph` without LLM or network
  access.
- Preserve exact source spans so every node and edge can be traced back to code.
- Represent uncertainty explicitly instead of hiding it.
- Keep domain, framework, and library meaning out of the core source lowerer.
- Support incremental recomputation by retaining deterministic IDs, provenance,
  and dependency facts.
- Let requirements, traceability, and sync features consume the same physical
  graph as call, control-flow, data-flow, and dependence views.

## Non-Goals

- Perfect dynamic dispatch resolution for all languages.
- Whole-program execution modeling.
- Runtime profiling or tracing.
- Domain-specific identification of business boundaries.
- Requirement generation inside the source lowerer.
- A second physical graph model or conversion step.

## Pipeline

The source analysis path has six stages:

1. Discover supported source artifacts.
2. Parse each supported file into `ProjectAst`.
3. Lower language-specific `ProjectAst` facts directly into
   `ProgramSupergraphBuilder`.
4. Resolve scopes, symbols, imports, call sites, and external targets as
   supergraph nodes and edges.
5. Run semantic enrichment over the resulting `ProgramSupergraph`.
6. Query call relationships, CFG, DFG, PDG, requirements, traceability, and
   invalidation through logical views.

Each stage emits facts with stable IDs derived from content, normalized paths,
source spans, owners, and fact family. This keeps graph output deterministic and
lets later invalidation logic identify affected slices.

The first language adapters use the existing Tree-sitter-based parser output as
their source. The adapter is not a second parser path; it translates `ProjectAst`
into shared supergraph facts.

## Core Supergraph Facts

### Source Artifact

A source artifact represents one file that can contribute scopes, callables,
calls, symbols, statements, expressions, and diagnostics.

Required payload:

- `artifact_id`: stable hash of normalized repository-relative path.
- `path`: repository-relative path.
- `language`: detected language identifier.
- `content_hash`: hash of file contents.
- `parser_version` and `analysis_version`: parser and adapter versions used to
  produce facts.

The builder stores artifacts as `GraphNode` records with `NodeFact::Artifact`.

An artifact also records the containers it belongs to (see below):
`directory_id`, `package_id`, `crate_id` (Rust) and `import_package_id`
(Python). All are plain references, not edges.

### Container

A container is a unit above the file level. Containers are stored as
`GraphNode` records with `NodeFact::Container`, with no owning artifact. Two
relations are recorded on each one, both as plain ids so that edges between
containers can be derived later:

- `directory_id`: the directory container where it lives (`None` for
  directories themselves and for manifests above the analysis root).
- `parent_container_id`: its logical parent.

| Kind | Source | Logical parent |
| --- | --- | --- |
| `Directory` | every directory holding a source file or a manifest, plus the root | parent directory |
| `Workspace` | Cargo `[workspace]`, npm/yarn `workspaces`, `pnpm-workspace.yaml`, `[tool.uv.workspace]` | none |
| `Package` | Cargo `[package]`, `package.json`, `pyproject.toml` (`[project]`, Poetry), `setup.cfg`, `setup.py` | the workspace whose members select it |
| `Crate` | one per Cargo target: lib, bin, example, test, bench (explicit or autodiscovered) | its package |
| `ImportPackage` | Python directory with `__init__.py` | parent import package, else the enclosing package |

Which manifests are read depends on the language: Rust reads Cargo, TypeScript
reads npm and pnpm, Python reads `pyproject.toml`, `setup.cfg` and `setup.py`
(first one declaring a package wins per directory). Manifests are searched under
the analysis root and in enclosing directories up to the repository root (the
nearest directory with `.git`). A manifest that fails to parse is skipped.

Payload beyond the identifying fields:

- `name`, `path` (directory relative to the analysis root, `.` for the root and
  `..`-prefixed above it), `manifest_path`, `version`, `ecosystem`.
- Crates: `target_kind`, `root_artifact_id` and `module_path` of the root file.
- Import packages: `root_artifact_id` and `module_path` of `__init__.py`.
- Packages: `entry_files` (npm `main`, `module`, `bin`) and `dependencies`.
  Each dependency has `name`, `rename`, `requirement`, `kind`, `optional`,
  `path`, `workspace`, `group`, and `resolved_container_id` when it names a
  package in the analyzed project (by path, else by unique name).
- Workspaces: `workspace_members` and `workspace_exclude` patterns as written.

Membership rules:

- `package_id`: the package whose directory most specifically contains the file.
- `crate_id`: Rust only. `lib.rs` and `main.rs` own their directory subtree; any
  other target root owns itself and the directory named after it. The most
  specific claim wins, then lib before bin before example, test, bench. This is
  a layout heuristic, not a resolution of `mod` declarations, so a file reached
  through several crates is attributed to one of them.
- `import_package_id`: Python only, set when the file's own directory has an
  `__init__.py`.

Not covered: TypeScript project references and `tsconfig` paths, Python
packages listed in build backends (`tool.setuptools`, Hatch, Poetry
`packages`), Cargo features and `[patch]`, and inline `mod` blocks.

### Source Span

Every extracted fact should keep an exact location.

Required payload:

- `artifact_id`
- `start_byte`
- `end_byte`
- `start_line`
- `start_column`
- `end_line`
- `end_column`

Byte offsets are canonical. Line and column positions support human-readable
output and diagnostics.

### Callable

A callable is a function, method, constructor, closure, lambda, property getter,
module initializer, or other executable code unit.

Required payload:

- `callable_id`: stable identity derived from language, path, lexical owner,
  callable name, declaration span, and signature shape.
- `kind`: `function`, `method`, `constructor`, `closure`, `lambda`,
  `property_getter`, `property_setter`, `module_initializer`, or
  `unknown_callable`.
- `name`: local name as written in source, if any.
- `qualified_name`: best static name within the project namespace.
- `artifact_id`
- `declaration_span`
- `body_span`
- `signature`: normalized parameters, receiver, return annotation, and
  visibility when available.
- `scope_id`: lexical scope that owns the callable.
- `attributes`: deterministic structural metadata such as decorators,
  annotations, modifiers, async markers, or receiver type.

Callable identity should remain stable across edits outside the callable. If a
declaration moves within the same file, the lowerer may use name and lexical
owner to preserve identity; if ambiguity remains, it should emit an identity
diagnostic rather than guess silently.

### Scope

Scopes model where symbols can be resolved.

Required payload:

- `scope_id`
- `parent_scope_id`
- `artifact_id`
- `kind`: `module`, `class`, `function`, `block`, or a language-specific scope
  kind normalized to the closest shared kind.
- `owner_callable_id`, when the scope belongs to a callable.
- `span`

Bindings are connected to scopes with `Binds` edges rather than nested inside a
separate scope object.

### Binding

A binding connects a name to one or more possible declarations or values.

Required payload:

- `binding_id`
- `scope_id`
- `name`
- `kind`: `import`, `assignment`, `parameter`, `class`, `function`, `method`,
  `field`, `module`, `external`, or `unknown`.
- `target`: declaration reference, imported module path, external symbol name,
  value reference, or unresolved placeholder.
- `span`
- `confidence`

When a binding has a deterministic target, emit a `ResolvesTo` edge to the
target node. Unresolved or ambiguous bindings should keep diagnostics and
uncertainty rather than inventing a target.

### Call Site

A call site represents one syntactic expression that may invoke behavior.

Required payload:

- `call_site_id`: stable hash of artifact, enclosing callable, and expression
  span.
- `artifact_id`
- `caller_callable_id`, using the module initializer callable for top-level
  calls.
- `span`
- `callee_expression`: normalized expression being called.
- `argument_shape`: positional count, named argument names, spread/rest usage,
  and obvious literal type hints when available.
- `dispatch_kind`: `direct`, `method`, `constructor`, `higher_order`,
  `decorator`, `operator`, `implicit`, or `unknown`.

Call sites are `NodeFact::CallSite` records and are contained by their caller
callable through `Contains`.

### Calls Edge

A `Calls` edge records the invocation relationship derived from a call site.

Required payload:

- `call_site_id`
- `caller_callable_id`
- `callee_callable_id`, when resolved to a local callable.
- `external_target_id`, when resolved to a third-party, standard library,
  runtime, framework, generated, or otherwise outside-project target.
- `unresolved_target`, when no concrete callable or external target can be
  selected.
- `kind`: `direct`, `method`, `constructor`, `decorator`, `implicit`,
  `possible_dynamic`, or `external`.
- `resolution`: `exact`, `overloaded`, `possible`, `external`, or `unresolved`.
- `evidence`: symbols, imports, receiver hints, resolution steps, and spans used
  to create the edge.

The supergraph edge source is the call site. The edge target is the local
callable or external target when a concrete target exists. Unresolved calls and
dynamic non-callable calls are targetless and keep their target text in the
payload. Dropping unresolved or external calls would make later views appear more
complete than the deterministic evidence allows.

### External Target

An external target represents callable-like behavior outside the analyzed
project. This includes third-party packages, standard libraries, language
runtimes, frameworks, generated code not present locally, and unresolved members
of otherwise known external modules.

Required payload:

- `external_target_id`: stable identity derived from package/module namespace,
  qualified symbol, target kind, and resolution evidence.
- `ecosystem`: package ecosystem when known, such as `pypi`, `crates`, `npm`,
  `stdlib`, `runtime`, or `unknown`.
- `package_name`: distribution, crate, or package name when known.
- `package_version`: installed or locked version when deterministically known.
- `module_path`: import path or namespace path.
- `qualified_name`: best known external callable name.
- `member_path`: attribute chain beyond the imported root, if applicable.
- `target_kind`: `function`, `method`, `constructor`, `decorator`,
  `class_callable`, `module_callable`, `operator`, or `unknown`.
- `source`: how the target was identified, such as `import`, `stdlib_catalog`,
  `lockfile`, `type_stub`, `manifest`, or `syntax_only`.

For example, `datetime.now(UTC)` should resolve to an external target for
`datetime.datetime.now` when the import and attribute chain are known.
`HTTPException(...)` should resolve to a constructor or class-callable target
from FastAPI when the import binding identifies it.

### Third-Party Call Context

External call facts should preserve enough context to answer questions such as
"show every third-party function call and where it happens."

The call view should be able to aggregate by:

- Package or ecosystem.
- Module path.
- Qualified external symbol.
- Caller callable.
- Source artifact.
- Edge kind, such as decorator, constructor, method, or direct call.
- Resolution confidence.
- Reachability from an entrypoint or boundary candidate.

Each result should be able to include:

- The caller callable and its source span.
- The exact call site span.
- The original callee expression as written.
- Argument shape.
- Import or binding evidence used for attribution.
- Whether the call is inside normal function body execution, module
  initialization, decorator evaluation, exception handling, a comprehension, or
  another adapter-identified context.

## Normalized Source Lowering

Each language adapter owns AST-specific traversal and writes normalized facts:

- Source artifacts.
- Callable declarations.
- Lexical scopes.
- Bindings.
- Imports and exports.
- Type declarations or class declarations.
- Statements, expressions, definitions, and uses that later CFG/DFG passes need.
- Call sites.
- External target candidates from imports, manifests, lockfiles, type stubs, or
  syntax-only module references.
- Structural attributes such as decorators, annotations, async markers, and
  receiver information.

The adapter must not decide business meaning. A Python adapter may record that a
function has an `app.get("/health")` decorator, but it must not declare the
function a business boundary. A later requirement layer can interpret that
metadata if configured to do so.

## Symbol And Scope Resolution

Symbol resolution runs after source files have contributed base facts.

For each artifact:

1. Create a module scope and module initializer callable.
2. Add imports to the module scope.
3. Add top-level classes, functions, constants, and assignments.
4. Create child scopes for classes and callables.
5. Add parameters and local assignments to callable scopes.
6. Add method bindings to class scopes.
7. Connect exports or public names according to language rules.

For the repository:

1. Build a project namespace from source roots and package/module declarations.
2. Resolve intra-project imports to artifact IDs and exported bindings.
3. Mark imports outside the project as external and attach package/module
   metadata when available.
4. Emit unresolved import diagnostics.

Symbol tables must be deterministic. If two files define the same qualified
symbol, the graph should retain both candidates and mark affected resolutions as
`overloaded` or `possible`.

## Call Resolution Algorithm

Call resolution consumes call sites, scopes, bindings, imports, class summaries,
and receiver facts already present in the source-lowering state or supergraph.

For every call site:

1. Identify the enclosing caller callable.
2. Classify the callee expression.
3. Resolve the root name in lexical scope.
4. Follow imports, aliases, class scopes, and member bindings.
5. Use receiver information when available.
6. Apply language-specific static rules for constructors, decorators,
   operators, async calls, and callable objects.
7. Emit `ResolvesTo` evidence where useful.
8. Emit one exact local `Calls` edge, multiple possible local `Calls` edges, an
   external `Calls` edge, or a targetless unresolved/dynamic `Calls` edge.

Resolution should prefer soundness over false precision. When a call might
target multiple local callables, emit multiple `possible` call facts from the
same call site instead of selecting one arbitrary target.

## Expression Resolution Rules

### Direct Name Calls

Example: `calculate_initial_risk(payload)`

Resolution:

1. Look up `calculate_initial_risk` in the current lexical scope.
2. Walk parent scopes if not found.
3. Follow import bindings if the name is imported.
4. If the final target is a local callable, emit an exact direct call fact.
5. If the final target is external, emit an external call fact.
6. If the name cannot be resolved, emit a targetless unresolved call fact.

### Attribute Or Method Calls

Example: `self._repository.create(payload, risk_score)`

Resolution:

1. Resolve the root expression, such as `self`.
2. Infer the receiver type from class scope, constructor assignments, type
   annotations, and direct assignments when available.
3. Resolve each attribute step against known class fields, module exports, or
   assignment bindings.
4. Resolve the final method or callable attribute.
5. Emit `exact` only when the receiver and member target are unambiguous.
6. Emit `possible` when receiver type or member target has multiple candidates.

The resolver should support shallow field flow within constructors because it is
common and deterministic. For example, if `__init__(repository:
IncidentRepository)` assigns `self._repository = repository`, then calls to
`self._repository.get(...)` can resolve to `IncidentRepository.get`.

### Constructor Calls

Example: `IncidentRecord(...)`

Resolution:

1. Resolve the name to a class binding.
2. If the class has an explicit constructor callable, target it.
3. Otherwise target the class's implicit constructor node.

The lowerer may create implicit constructor nodes so class construction remains
traceable even when no constructor method is declared.

### Decorator Calls

Example: `@app.get("/health")`

Decorators should be represented in two ways:

- As attributes on the decorated callable.
- As call sites in the module initializer, because evaluating decorators is
  executable behavior in languages such as Python.

If the decorator target resolves locally, emit a decorator call fact. If it is a
framework call outside the project, emit an external call fact and retain
decorated callable external invocation metadata.

### Higher-Order Calls

Example: `callback(record)`

Resolution:

1. Resolve the called name.
2. If the binding is a parameter or variable with callable type information,
   emit possible call facts to known assignments.
3. If no deterministic target exists, emit a targetless unresolved higher-order
   call fact.

The initial implementation should preserve these call sites without attempting
deep interprocedural value flow.

### Comprehensions And Closures

Comprehensions, lambdas, closures, and anonymous functions should be represented
as callable nodes when they contain calls or non-trivial behavior. Their
generated names must be deterministic, such as
`IncidentService.list_incidents::<listcomp@body-span>`.

### Implicit Calls

Some syntax invokes behavior without an explicit call expression: operators,
iteration protocols, context managers, property access, derives/macros, or
language-specific magic methods.

The first implementation should:

- Record implicit call sites only when the language adapter can identify them
  deterministically.
- Mark them as `implicit`.
- Avoid speculative call facts unless the target is statically known.

## Python Adapter Initial Scope

The Python example under `doc/example/python/incident_triage` is the primary
source fixture.

The Python adapter should support:

- Modules and package-relative imports.
- Top-level functions.
- Classes and instance methods.
- `self` receiver resolution.
- Constructor field flow from `__init__` assignments.
- Function calls by direct name.
- Method calls through `self`, local variables, imported module names, and
  module-level instances.
- Class construction calls.
- Decorator call sites and decorator metadata.
- List comprehensions as anonymous callable nodes when they contain calls.
- Exception construction calls.

The first version may defer:

- Monkey patching.
- Dynamic imports.
- Metaclasses.
- Descriptor protocol precision.
- Full union type narrowing.
- Deep interprocedural data flow.

## Example: Incident Triage Call Facts

From the fixture, `CallGraphView` should expose relationships like:

- `app.main.report_incident` calls
  `app.service.IncidentService.report_incident`.
- `app.service.IncidentService.report_incident` calls
  `app.risk.calculate_initial_risk`.
- `app.service.IncidentService.report_incident` calls
  `app.repository.IncidentRepository.create`.
- `app.service.IncidentService.update_incident` calls
  `app.repository.IncidentRepository.get`.
- `app.service.IncidentService.update_incident` calls
  `app.service.IncidentService._ensure_transition_allowed`.
- `app.service.IncidentService.update_incident` calls
  `app.risk.recalculate_risk`.
- `app.service.IncidentService.update_incident` calls
  `app.repository.IncidentRepository.save`.
- `app.service.IncidentService.triage_incident` calls
  `app.repository.IncidentRepository.get`.
- `app.service.IncidentService.triage_incident` calls
  `app.risk.requires_escalation`.
- `app.service.IncidentService.triage_incident` calls
  `app.service.IncidentService._select_channel`.
- `app.service.IncidentService.triage_incident` calls
  `app.service.IncidentService._build_rationale`.
- `app.risk.calculate_initial_risk` calls
  `app.risk._customer_impact_weight`.
- `app.risk.calculate_initial_risk` calls `app.risk._region_spread_weight`.
- `app.repository.IncidentRepository.update` calls
  `app.repository.IncidentRepository.get`.

The resolver should also preserve external call facts such as:

- route decorator calls to FastAPI registration methods.
- `HTTPException(...)`.
- `datetime.now(...)`.
- `uuid4(...)`.
- Pydantic `model_copy(...)` calls unless resolved as external methods.

## Entrypoint Detection

Call-oriented entrypoint candidates are derived from supergraph call facts. A
callable with no incoming local call facts may be a structural entrypoint.

Structural entrypoints are not always business entrypoints. For example, FastAPI
route functions have incoming runtime calls from the framework that do not
appear as local source calls. The supergraph should therefore preserve:

- `incoming_local_call_count` on callable payloads when serialized behavior
  needs it.
- `external_invocation_metadata` from decorators, exports, or adapter-supported
  framework facts.

The requirements layer can then decide whether a callable is an `entrypoint`
using deterministic graph structure plus separately stored domain or framework
metadata.

## Boundary Candidate Support

Source lowering should not decide that a callable is a `boundary`. It should
provide evidence that later layers can use:

- Public visibility or export status.
- Decorators and annotations.
- External invocation metadata.
- Calls crossing module/package ownership boundaries.
- Calls to external systems or framework APIs.
- Fan-in and fan-out counts from logical call views.
- Reachability from structural entrypoints.

## Incremental Updates

The engine should cache parser and analysis outputs by content hash, parser
version, and analysis version.

When one file changes:

1. Reparse only that artifact.
2. Re-lower facts for that artifact.
3. Rebuild symbol tables for affected scopes and imports.
4. Re-resolve calls whose lexical scope, import target, or candidate callee set
   changed.
5. Recompute graph indexes and semantic facts affected by the changed slice.

The dependency and invalidation views must track:

- Artifact imports.
- Exported symbol references.
- Callable identity dependencies.
- Call site resolution dependencies.
- Semantic enrichment dependencies such as CFG, DFG, PDG, requirement, and
  traceability facts.

This keeps normal editor feedback fast enough for requirements traceability.

## Diagnostics

Diagnostics are first-class supergraph facts. They prevent the graph from
implying false certainty.

Important diagnostic kinds:

- `parse_error`
- `unsupported_syntax`
- `unresolved_import`
- `unresolved_symbol`
- `external_target_unknown_package`
- `external_target_ambiguous`
- `ambiguous_symbol`
- `dynamic_dispatch`
- `callable_identity_churn`
- `adapter_internal_error`

Each diagnostic should include source span, severity, message, and related facts
or candidate IDs when available.

## Testing Strategy

Tests should assert `ProgramSupergraph` facts and logical view behavior.

Required test layers:

- Parser and source-lowering tests per language adapter.
- Symbol table tests for imports, aliases, scopes, classes, and methods.
- Call resolution tests for direct calls, method calls, constructors,
  decorators, unresolved calls, and external calls.
- External target tests that verify package/module attribution and third-party
  call aggregation context.
- Golden supergraph tests against the incident triage fixture.
- `CallGraphView` tests for local, external, unresolved, all-call, caller,
  call-site, and concrete-target traversal.
- Incremental invalidation tests that edit one file and assert the affected
  supergraph slice changes while unrelated identities remain stable.

Golden tests should compare structured supergraph records with stable ordering.
Markdown snapshots can be used only for human-facing renderers, not for the core
analysis output.

## Implementation Phases

### Phase 1: Source Lowering And Python Function Calls

- Define supergraph records and stable IDs for source-level facts.
- Parse Python files with the existing Tree-sitter Python adapter.
- Insert artifacts, scopes, callables, bindings, direct call sites, external
  targets, diagnostics, and call facts through `ProgramSupergraphBuilder`.
- Resolve direct name calls and simple imported functions.
- Expose output through the library API and supergraph CLI commands.

### Phase 2: Methods, Constructors, And Shallow Field Flow

- Add class scopes and method bindings.
- Resolve `self.method(...)`.
- Resolve constructor calls.
- Track constructor assignments such as `self._repository = repository`.
- Resolve calls through fields with annotated constructor parameters.

### Phase 3: Decorators And External Invocation Metadata

- Record decorators as metadata and executable call sites.
- Preserve external framework calls.
- Populate external invocation metadata for decorated callables.
- Derive structural entrypoint candidates from supergraph call facts.

### Phase 4: Semantic Enrichment

- Add CFG facts for callable-local execution ordering.
- Add control-dependence facts from post-dominance.
- Add DFG facts for definitions, uses, value movement, parameters, and returns.
- Add interprocedural data-flow facts across call boundaries.
- Keep every enriched fact in `ProgramSupergraph`.

### Phase 5: Incremental Analysis

- Add content-addressed stage caches.
- Track symbol, import, call-site, and semantic enrichment dependencies.
- Recompute affected graph slices after file edits.
- Preserve stable callable IDs across unrelated edits.

### Phase 6: Multi-Language Adapter Boundary

- Formalize the adapter trait/interface around direct supergraph lowering.
- Add a second language fixture.
- Prove shared resolution and semantic enrichment can consume facts from
  multiple adapters.
- Keep language-specific rules isolated behind adapter-provided metadata.

## Open Design Questions

- How should stable callable IDs behave when a function is renamed but the body
  is mostly unchanged?
- What is the minimum type inference needed before method resolution becomes
  useful enough for requirements traceability?
- Should unresolved external calls be grouped by package/module for readability
  in later human-facing outputs?
- Which incoming-call and external-invocation summaries should remain serialized
  callable payloads versus view/index-derived values?
