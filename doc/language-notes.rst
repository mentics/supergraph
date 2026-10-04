=====================================
Language-specific notes for Supergraph
=====================================

Supergraph lowers every supported language into one shared model: a
per-file ``FileAst`` (``src/ast.rs``) produced by a Tree-sitter frontend in
``src/parser/``, and a ``ProgramSupergraph`` built by a language adapter in
``src/analysis/source_graph/``. The shared model is shaped like Python and
TypeScript, so each language has to be fitted onto it. This document records
where that fitting is approximate, what is deliberately not modeled, and
which quirks a reader of the graph should know about.

Add a section here whenever a frontend takes a shortcut, picks a mapping that
is not obvious, or knowingly loses information.

.. contents::
   :local:
   :depth: 2


Conventions shared by all languages
===================================

Parse errors are fatal
    Each frontend returns an error if Tree-sitter reports any syntax error in
    a file, so one unparseable file fails the whole analysis of its project.

Module paths are dotted
    ``src/a/b.rs`` becomes ``src.a.b``. Symbol ids are ``<module>:<name>`` or
    ``<module>:<Parent>.<name>``; callable ids are ``<module>.<name>`` or
    ``<module>.<Parent>.<name>``.

Facts are syntactic
    The frontends extract what the syntax tree shows. There is no type
    inference, so anything that depends on a value's type (method dispatch,
    trait resolution, overloads) is resolved by name or left unresolved.

Unresolved is a valid result
    A call that cannot be bound becomes an ``UnresolvedTarget`` with evidence,
    not an error.


Rust
====

Frontend: ``src/parser/rust.rs`` (``tree-sitter-rust``). Adapter:
``src/analysis/source_graph/rust.rs``. Language name in the graph: ``rust``.

Mapping onto the shared model
-----------------------------

Classes
    ``struct``, ``enum``, ``union`` and ``trait`` items become ``Class``
    symbols. They carry no body facts; their fields and variants are not
    modeled.

Methods
    Functions inside an ``impl`` block become ``Method`` symbols whose parent
    is the bare type name of the ``impl`` target (``Foo<T>`` and ``&Foo``
    become ``Foo``). ``impl Trait for Type`` attaches its methods to ``Type``.
    A trait's default methods attach to the trait. Bodiless trait method
    signatures are not callables.

Constructors
    Rust has none, so the adapter reports no constructor method name.
    ``Type::new`` is an ordinary associated function. Every class still gets
    the implicit constructor the shared lowerer creates (id
    ``<class>.<implicit>``); it is the target of tuple-struct construction
    such as ``Wrapper(1)``. Struct literals (``Foo { .. }``) are not calls.

Decorators
    Outer attributes (``#[derive(Debug)]``) that immediately precede an item
    are reported as its decorators, with the ``#[`` and ``]`` stripped.
    Inner attributes (``#![..]``) are ignored.

Raises
    ``panic!``, ``unreachable!``, ``todo!`` and ``unimplemented!`` are
    ``Raise`` statements and raise facts. ``?`` does not raise; an early
    return through ``?`` is not modeled. ``assert!`` and friends are plain
    macro calls.

Switch-like constructs
    ``match`` is emitted as an ``Unknown`` statement, the same fallback
    TypeScript uses for ``switch``. Arms are not modeled as branches, so
    control flow through a ``match`` is approximate.

Imports
    ``use`` trees are flattened to one ``ImportAst`` per imported name.
    ``use a::{b, c as d}`` becomes module ``a`` with names ``b`` and ``c``
    (alias ``d``). ``use a::b::{self}`` imports module ``a::b``. A bare
    ``use serde;`` has no module. Glob imports (``use a::*``) carry a module
    and no names, so the lowerer records them as unresolved imports and no
    names are bound.

Module resolution
-----------------

* A file's module path is the path of the module it defines. ``lib.rs``,
  ``main.rs`` and ``mod.rs`` name their containing directory
  (``src/lib.rs`` is ``src``; ``src/shapes/mod.rs`` is ``src.shapes``).
  ``foo.rs`` and ``foo/mod.rs`` are the same module, so ``self`` and ``super``
  work by the same arithmetic for both layouts.
* ``crate`` resolves to the first ``src`` segment of the importing module's
  path, or to the analysis root if there is none. A workspace analyzed from
  its root works because each crate's ``src`` directory is found per file;
  two crates that share module names under different ``src`` directories are
  kept apart by that prefix.
* Paths that start with neither ``crate``, ``self`` nor ``super`` are tried
  relative to the importing module, then relative to the crate root. If
  neither is a project module the path is left as written and treated as
  external (``std``, ``core``, ``alloc`` are ecosystem ``runtime``; anything
  else is ``cargo``).
* ``Cargo.toml`` is never read. Crate names, ``[lib] path`` overrides,
  ``#[path = ".."]`` attributes and ``extern crate`` renames are not
  understood, so a dependency that shares a name with a local top-level module
  resolves to the local module.

Call resolution
---------------

Callee text uses two forms:

* ``receiver.method(..)`` has a receiver and callee ``receiver.method``.
* ``Path::item(..)`` has no receiver and callee ``Path::item``.

``self.method()`` resolves against the enclosing type's methods.
``Type::f()``, ``Self::f()`` and ``module::f()`` resolve through the type's
methods, imports, or fully qualified project paths. Other
``receiver.method()`` calls resolve only when the receiver is a module-level
``const``/``static`` initialized from ``Type::ctor(..)`` or ``Type { .. }``;
otherwise they are unresolved. In particular:

* There is no type inference, so ``let x = Foo::new(); x.run()`` does not
  resolve ``run``.
* Trait method dispatch is not resolved. If two traits implemented on one
  type define the same method name, the later symbol shadows the earlier in
  the type's method table.
* Generic and ``dyn Trait`` dispatch are not resolved. Calls through function
  pointers and closures held in variables look like plain calls to a local
  name and resolve only if a function of that name exists.
* A turbofish is dropped: ``parse::<u32>()`` has callee ``parse``.
* Macro invocations are calls with callee ``name!``, zero arguments and an
  external ``rust.macros`` target. Macro bodies are token trees, so
  identifiers inside them are recorded as uses but nothing else is
  analyzed. User-defined macros and ``macro_rules!`` definitions are not
  expanded.
* Prelude constructors (``Some``, ``Ok``, ``Err``) and a few prelude items
  resolve to an external ``std.prelude`` target.

Returns
-------

The final unterminated expression of a function body is its implicit return
value. The frontend adds a ``Return`` statement and return fact for it when it
is a plain value expression. It does not when the tail is an ``if``,
``match``, loop, block, ``return`` or panic, because those already have their
own statements.

Not modeled: implicit returns from nested blocks (for example the tail of an
``if`` branch used as a value), and closure bodies' implicit returns.

Structural simplifications
--------------------------

* Inline ``mod name { .. }`` blocks are flattened into the enclosing file's
  module. Items in different inline modules of one file share a namespace,
  so same-named items collide.
* Closures are not symbols. Calls inside a closure body are attributed to the
  enclosing function, and the closure body is skipped when collecting that
  function's statements and definitions (matching how arrow functions are
  treated for TypeScript).
* Nested ``fn`` items inside a function body are not separate callables; they
  appear as ``Function`` statements only.
* ``async`` functions are ordinary functions, ``.await`` is an ``Await``
  expression, and ``async`` blocks are not separate scopes. Futures are not
  modeled.
* Lifetimes, generics, where-clauses and type annotations are not analyzed.
  Return types are kept as source text.
* ``unsafe`` blocks, ``const`` blocks and labeled blocks are plain blocks.
  Labels on ``break``/``continue`` are ignored.
* Ownership, borrowing and moves are not modeled, so data-flow treats
  ``let b = a;`` as a copy and ignores that ``a`` is no longer usable.
  ``&`` and ``*`` are not distinguished from other unary operators.
* ``loop`` has no condition fact, and a ``for`` loop's condition fact is its
  iterable expression.

Bindings and patterns
---------------------

Pattern definitions (``let``, ``for``, ``match`` arms, ``if let``,
``while let``) collect every bound identifier. The collector cannot tell a
binding from a constant by syntax, so identifiers that start with an
uppercase letter are assumed to be enum variants or constants and are not
treated as bindings. A ``const`` named in snake case inside a pattern would be
misread as a binding; an uppercase local variable would be missed.

Shadowing (``let x = ..; let x = ..;``) produces one definition per ``let``;
whether later uses see the first or second is left to the shared scope
analysis, which models blocks as scopes (``rust:block``).

Conditions and expressions
--------------------------

* ``if`` expressions always produce an ``If`` statement and condition. An
  ``if`` used for its value (``let x = if c { a } else { b };``) additionally
  produces a ``Conditional`` expression. ``else if`` chains are nested
  ``if`` expressions, not ``ElseIf`` conditions.
* An ``if let`` condition is the whole ``let pat = value`` text.
* Expression statements that wrap control flow or a panic are not also
  reported as ``Expression`` statements, to avoid counting them twice.
* Field and index accesses use Tree-sitter field names where they exist;
  ``index_expression`` has none, so the first and second named children are
  taken as object and index.
* ``?`` (try) and ``as`` casts are not given an expression kind.

Files and discovery
-------------------

``discover_rust_files`` takes every ``*.rs`` file under the root and skips
directories named ``target`` and ``.git`` below the root. Generated code under
other names (for example ``OUT_DIR`` copies) is included. ``build.rs`` files
are analyzed like any other module.


TypeScript
==========

Frontend: ``src/parser/typescript.rs``. Adapter:
``src/analysis/source_graph/typescript.rs``.

* ``.tsx`` files use the TSX grammar; ``.ts`` files use the plain TypeScript
  grammar. ``node_modules`` is skipped.
* ``import`` statements are parsed by string splitting on the statement text,
  not from the syntax tree, so unusual forms (default plus named imports in
  one statement, namespace imports with other clauses) may be parsed
  incompletely.
* Top-level ``const f = () => ..`` and ``const f = function ..`` are treated
  as functions; arrow functions elsewhere are not symbols.
* A capitalized callee is treated as a direct call (a constructor or JSX
  component); JSX elements with capitalized names are recorded as calls.
* The adapter recognizes a small fixed set of JavaScript builtins
  (``Math``, ``Array`` methods ``map``/``filter``/``reduce``/``forEach``/
  ``find``) and resolves them to external targets. Other runtime members are
  unresolved.
* ``switch`` is emitted as an ``Unknown`` statement.
* ``tsconfig`` path aliases and package ``exports`` are not read; only
  relative imports (``./``, ``../``) resolve to project modules, with an
  ``index`` fallback for directories.


Python
======

Frontend: ``src/parser/python.rs``. Adapter:
``src/analysis/source_graph/python.rs``.

* Block scopes are transparent for bindings, matching Python's function-level
  scoping; ``except`` handlers have language-specific binding behavior.
* Comprehension scopes are detected from the expression text (a leading
  bracket plus `` for `` and `` in ``), so unusual spacing or nesting can be
  missed.
* Only ``.py`` files are discovered; stub files and notebooks are not.
