use crate::intern::Sym;
use std::collections::BTreeSet;

use crate::analysis::enrich_supergraph_with_semantic_flows;
use crate::ast::{AssignmentAst, FileAst, ProjectAst};
use crate::parser::rust::module_path as rust_module_path;
use crate::supergraph::{
    BindingTarget, CallEdgeKind, Confidence, ExternalTarget, ExternalTargetKind, ProgramSupergraph,
    stable_id,
};
use crate::id_parts;
use crate::supergraph::ids::{NodeId, Tag};

use super::adapter::{
    GraphContext, ImportBinding, LanguageSupergraphAdapter, PendingCall, ResolvedCall,
};
use super::build_initial_supergraph;

const ANALYSIS_VERSION: &str = "rust-tree-sitter.v1";

pub(crate) fn build_rust_supergraph(project: &ProjectAst) -> ProgramSupergraph {
    enrich_supergraph_with_semantic_flows(
        build_initial_supergraph(project, RustSupergraphAdapter),
        project,
    )
}

#[derive(Debug, Clone, Copy)]
struct RustSupergraphAdapter;

impl RustSupergraphAdapter {
    fn resolve_imported_call(
        &self,
        expression: &str,
        import: &ImportBinding,
        edge_kind: CallEdgeKind,
        context: &GraphContext,
    ) -> ResolvedCall {
        match &import.target {
            BindingTarget::Callable(callee) => ResolvedCall::LocalTarget {
                callee: callee.clone(),
                kind: edge_kind,
                evidence: vec![format!("import binding: {expression}")],
            },
            // Tuple structs are constructed with call syntax: `Wrapper(1)`.
            BindingTarget::Class(class_name) => match context.classes.get(class_name.as_str()) {
                Some(class) => ResolvedCall::LocalTarget {
                    callee: class
                        .explicit_constructor_id
                        .clone()
                        .unwrap_or_else(|| class.implicit_constructor_id.clone()),
                    kind: CallEdgeKind::Constructor,
                    evidence: vec![format!("imported type constructor: {class_name}")],
                },
                None => ResolvedCall::UnresolvedTarget {
                    target: expression.to_string(),
                    evidence: vec![format!("missing type for import {class_name}")],
                },
            },
            BindingTarget::External(external) => ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some(&import.module_path),
                    external,
                    import.imported_name.as_deref(),
                    self.external_kind_for(edge_kind),
                    "import",
                    Confidence::Exact,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Exact,
                evidence: vec![format!(
                    "external import binding: {expression} -> {external}"
                )],
            },
            _ => ResolvedCall::UnresolvedTarget {
                target: expression.to_string(),
                evidence: vec![format!("unsupported import binding for {expression}")],
            },
        }
    }

    /// Method call syntax: `receiver.member(..)`.
    fn resolve_method_call(
        &self,
        pending: &PendingCall,
        receiver: &str,
        context: &GraphContext,
    ) -> ResolvedCall {
        let expression = pending.call.callee.as_str();
        let member = expression.rsplit('.').next().unwrap_or(expression);

        if receiver == "self" {
            if let Some(class_name) = &pending.class_qualified_name {
                if let Some(callee) = context
                    .classes
                    .get(class_name.as_str())
                    .and_then(|class| class.methods.get(member))
                {
                    return ResolvedCall::LocalTarget {
                        callee: callee.clone(),
                        kind: CallEdgeKind::Method,
                        evidence: vec![format!("self method on {class_name}")],
                    };
                }
            }
        }

        if let Some(file_context) = context.file_context_for_module(&pending.module_path) {
            if let Some(instance_class) = file_context.assignments.get(receiver) {
                if let Some(callee) = context
                    .classes
                    .get(instance_class)
                    .and_then(|class| class.methods.get(member))
                {
                    return ResolvedCall::LocalTarget {
                        callee: callee.clone(),
                        kind: CallEdgeKind::Method,
                        evidence: vec![format!(
                            "module assignment: {receiver} -> {instance_class}"
                        )],
                    };
                }
            }
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no receiver binding found for {receiver}")],
        }
    }

    /// Path call syntax: `Type::member(..)` or `module::member(..)`.
    fn resolve_path_call(
        &self,
        pending: &PendingCall,
        path: &str,
        member: &str,
        edge_kind: CallEdgeKind,
        context: &GraphContext,
    ) -> ResolvedCall {
        let expression = pending.call.callee.as_str();
        let segments = path.split("::").collect::<Vec<_>>();
        let dotted = segments.join(".");

        if path == "Self" {
            if let Some(class_name) = &pending.class_qualified_name {
                if let Some(callee) = context
                    .classes
                    .get(class_name.as_str())
                    .and_then(|class| class.methods.get(member))
                {
                    return ResolvedCall::LocalTarget {
                        callee: callee.clone(),
                        kind: CallEdgeKind::Method,
                        evidence: vec![format!("Self associated function on {class_name}")],
                    };
                }
            }
        }

        if let [name] = segments.as_slice() {
            if let Some(callee) = context
                .resolve_class_name(&pending.module_path, name)
                .and_then(|class| class.methods.get(member))
            {
                return ResolvedCall::LocalTarget {
                    callee: callee.clone(),
                    kind: CallEdgeKind::Method,
                    evidence: vec![format!("associated function: {name}::{member}")],
                };
            }

            if let Some(import) = context
                .file_context_for_module(&pending.module_path)
                .and_then(|file_context| file_context.imports.get(*name))
            {
                match &import.target {
                    BindingTarget::Class(class_name) => {
                        if let Some(callee) = context
                            .classes
                            .get(class_name.as_str())
                            .and_then(|class| class.methods.get(member))
                        {
                            return ResolvedCall::LocalTarget {
                                callee: callee.clone(),
                                kind: CallEdgeKind::Method,
                                evidence: vec![format!(
                                    "imported associated function: {class_name}::{member}"
                                )],
                            };
                        }
                    }
                    BindingTarget::Module(module) => {
                        if let Some(callee) =
                            context.local_callables.get(&format!("{module}.{member}"))
                        {
                            return ResolvedCall::LocalTarget {
                                callee: callee.clone(),
                                kind: edge_kind,
                                evidence: vec![format!("imported module member: {module}.{member}")],
                            };
                        }
                    }
                    BindingTarget::External(external) => {
                        return ResolvedCall::ExternalTarget {
                            target: self.external_target(
                                Some(&import.module_path),
                                &format!("{external}.{member}"),
                                Some(member),
                                self.external_kind_for(edge_kind),
                                "import",
                                Confidence::Probable,
                            ),
                            kind: CallEdgeKind::External,
                            confidence: Confidence::Probable,
                            evidence: vec![format!("external import: {name} -> {external}")],
                        };
                    }
                    _ => {}
                }
            }
        }

        // Fully qualified paths: `crate::util::helper()`, `util::Type::new()`.
        let normalized = self.normalize_import_module(
            &pending.module_path,
            &dotted,
            &context.local_modules.clone(),
        );
        if let Some(callee) = context.local_callables.get(&format!("{normalized}.{member}")) {
            return ResolvedCall::LocalTarget {
                callee: callee.clone(),
                kind: edge_kind,
                evidence: vec![format!("qualified path: {normalized}.{member}")],
            };
        }
        if let Some(callee) = context
            .classes
            .get(&normalized)
            .and_then(|class| class.methods.get(member))
        {
            return ResolvedCall::LocalTarget {
                callee: callee.clone(),
                kind: CallEdgeKind::Method,
                evidence: vec![format!("qualified associated function: {normalized}::{member}")],
            };
        }

        if is_external_root(segments[0], context) {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some(&dotted),
                    &format!("{dotted}.{member}"),
                    Some(member),
                    self.external_kind_for(edge_kind),
                    "path",
                    Confidence::Probable,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Probable,
                evidence: vec![format!("external path: {expression}")],
            };
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no binding found for path {path}")],
        }
    }
}

impl LanguageSupergraphAdapter for RustSupergraphAdapter {
    fn language(&self) -> &'static str {
        "rust"
    }

    fn analysis_version(&self) -> &'static str {
        ANALYSIS_VERSION
    }

    fn module_path(&self, _project: &ProjectAst, relative_path: &str) -> String {
        rust_module_path(relative_path)
    }

    /// Resolve `crate`, `self` and `super` prefixes and 2018-style relative
    /// paths against the project's modules. Because a module's path is the
    /// path of the module itself (`foo.rs` and `foo/mod.rs` are both `foo`),
    /// the same arithmetic works for every file layout.
    fn normalize_import_module(
        &self,
        current_module: &str,
        import_module: &str,
        local_modules: &BTreeSet<String>,
    ) -> String {
        let current = split_module(current_module);
        let segments = split_module(import_module);
        let Some((first, rest)) = segments.split_first() else {
            return import_module.to_string();
        };

        let join = |base: &[String], rest: &[String]| {
            base.iter()
                .chain(rest.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(".")
        };

        match first.as_str() {
            "crate" => {
                // The crate root is the `src` directory when one is present.
                let root = current
                    .iter()
                    .position(|segment| segment == "src")
                    .map(|index| &current[..=index])
                    .unwrap_or(&[]);
                join(root, rest)
            }
            "self" => join(&current, rest),
            "super" => {
                let mut base = current.clone();
                let mut rest = rest;
                base.pop();
                while let Some((next, tail)) = rest.split_first() {
                    if next != "super" {
                        break;
                    }
                    base.pop();
                    rest = tail;
                }
                join(&base, rest)
            }
            _ => {
                let relative = join(&current, &segments);
                if local_modules.contains(&relative) {
                    return relative;
                }
                let root = current
                    .iter()
                    .position(|segment| segment == "src")
                    .map(|index| &current[..=index])
                    .unwrap_or(&[]);
                let rooted = join(root, &segments);
                if local_modules.contains(&rooted) {
                    return rooted;
                }
                import_module.to_string()
            }
        }
    }

    fn implicit_constructor_id(&self, class_name: &str) -> String {
        format!("{class_name}.<implicit>")
    }

    /// Rust has no constructors; `Type::new` is an ordinary associated
    /// function.
    fn constructor_method_name(&self) -> Option<&'static str> {
        None
    }

    fn external_target(
        &self,
        module_path: Option<&str>,
        qualified_name: &str,
        member_path: Option<&str>,
        target_kind: ExternalTargetKind,
        source: &str,
        confidence: Confidence,
    ) -> ExternalTarget {
        external_target(
            module_path,
            qualified_name,
            member_path,
            target_kind,
            source,
            confidence,
        )
    }

    /// `static FOO: T = Type::new(..)` and `const FOO: T = Type { .. }` bind
    /// `FOO` to `Type`.
    fn assignment_class_target(
        &self,
        module_path: &str,
        _file: &FileAst,
        assignment: &AssignmentAst,
        context: &GraphContext,
    ) -> Option<String> {
        let value = assignment.value.as_deref()?.trim();
        let head = value.split(['(', '{']).next()?.trim();
        let type_name = head.rsplit_once("::").map(|(path, _)| path).unwrap_or(head);
        let type_name = type_name.rsplit("::").next()?;
        context
            .resolve_class_name(module_path, type_name)
            .map(|class| class.qualified_name.clone())
    }

    fn resolve_call(
        &self,
        pending: &PendingCall,
        _call_site_id: NodeId,
        context: &GraphContext,
    ) -> ResolvedCall {
        let expression = pending.call.callee.as_str();
        let edge_kind = self.edge_kind(&pending.call);

        if let Some(receiver) = &pending.call.receiver {
            return self.resolve_method_call(pending, receiver, context);
        }

        if expression.ends_with('!') {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some("rust.macros"),
                    expression,
                    None,
                    ExternalTargetKind::Function,
                    "runtime",
                    Confidence::Probable,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Probable,
                evidence: vec![format!("macro invocation: {expression}")],
            };
        }

        if let Some((path, member)) = expression.rsplit_once("::") {
            return self.resolve_path_call(pending, path, member, edge_kind, context);
        }

        if let Some(callee) = context.resolve_direct_local(&pending.module_path, expression) {
            return ResolvedCall::LocalTarget {
                callee,
                kind: edge_kind,
                evidence: vec![format!("direct name lookup: {expression}")],
            };
        }

        // Tuple-struct construction: `Wrapper(1)`.
        if let Some(class) = context.resolve_class_name(&pending.module_path, expression) {
            return ResolvedCall::LocalTarget {
                callee: class
                    .explicit_constructor_id
                    .clone()
                    .unwrap_or_else(|| class.implicit_constructor_id.clone()),
                kind: CallEdgeKind::Constructor,
                evidence: vec![format!("tuple struct constructor: {}", class.qualified_name)],
            };
        }

        if let Some(file_context) = context.file_context_for_module(&pending.module_path) {
            if let Some(import) = file_context.imports.get(expression) {
                return self.resolve_imported_call(expression, import, edge_kind, context);
            }
        }

        if is_prelude_function(expression) {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some("std.prelude"),
                    expression,
                    None,
                    ExternalTargetKind::Function,
                    "runtime",
                    Confidence::Exact,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Exact,
                evidence: vec![format!("Rust prelude item: {expression}")],
            };
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no binding found for {expression}")],
        }
    }
}

fn split_module(module: &str) -> Vec<String> {
    module
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// A path rooted outside the project: the standard library or a dependency.
fn is_external_root(root: &str, context: &GraphContext) -> bool {
    !matches!(root, "crate" | "self" | "super")
        && !context
            .local_modules
            .iter()
            .any(|module| module.split('.').next() == Some(root))
}

fn external_target(
    module_path: Option<&str>,
    qualified_name: &str,
    member_path: Option<&str>,
    target_kind: ExternalTargetKind,
    source: &str,
    _confidence: Confidence,
) -> ExternalTarget {
    let module_path = module_path.map(str::to_string);
    let package_name = module_path
        .as_ref()
        .and_then(|module| module.split('.').next())
        .map(str::to_string);
    ExternalTarget {
        external_target_id: stable_id(Tag::External,
            id_parts![
                module_path.as_deref().unwrap_or("unknown"),
                qualified_name,
                member_path.unwrap_or(""),
                &format!("{target_kind:?}"),
            ],
        ),
        ecosystem: Sym::from(package_name
            .as_deref()
            .map(ecosystem_for_package)
            .unwrap_or("unknown")
            .to_string()),
        package_name: package_name.map(Sym::from),
        package_version: None,
        module_path: module_path.map(Sym::from),
        qualified_name: Sym::from(qualified_name.to_string()),
        member_path: (member_path.map(str::to_string)).map(Sym::from),
        target_kind,
        source: Sym::from(source.to_string()),
    }
}

fn ecosystem_for_package(package: &str) -> &'static str {
    match package {
        "std" | "core" | "alloc" | "rust" => "runtime",
        _ => "cargo",
    }
}

fn is_prelude_function(name: &str) -> bool {
    matches!(name, "Some" | "Ok" | "Err" | "drop" | "Box" | "Vec" | "String")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::rust::{filter_file, parse_rust_file};
    use crate::supergraph::CallGraphView;
    use crate::supergraph::ids::callable_id_from_text;

    fn project(files: &[(&str, &str)]) -> ProjectAst {
        let directory = std::env::temp_dir().join(format!(
            "supergraph-rust-{}-{}",
            std::process::id(),
            files.len()
        ));
        let mut parsed = Vec::new();
        for (path, source) in files {
            let absolute = directory.join(path);
            std::fs::create_dir_all(absolute.parent().expect("parent")).expect("mkdir");
            std::fs::write(&absolute, source).expect("write fixture");
            let file = parse_rust_file(&absolute, (*path).to_string()).expect("parse fixture");
            parsed.push(filter_file(&file));
        }
        let _ = std::fs::remove_dir_all(&directory);
        ProjectAst {
            root: "repo".to_string(),
            files: parsed,
        }
    }

    #[test]
    fn normalizes_crate_self_and_super_paths() {
        let modules = ["src", "src.shapes", "src.shapes.circle", "src.util"]
            .iter()
            .map(|module| module.to_string())
            .collect::<BTreeSet<_>>();
        let adapter = RustSupergraphAdapter;
        let normalize = |current: &str, import: &str| {
            adapter.normalize_import_module(current, import, &modules)
        };

        assert_eq!(normalize("src.shapes.circle", "crate.util"), "src.util");
        assert_eq!(normalize("src.shapes.circle", "super"), "src.shapes");
        assert_eq!(normalize("src.shapes", "self.circle"), "src.shapes.circle");
        assert_eq!(normalize("src.shapes.circle", "super.super.util"), "src.util");
        assert_eq!(normalize("src", "shapes"), "src.shapes");
        assert_eq!(normalize("src.shapes", "std.collections"), "std.collections");
    }

    #[test]
    fn resolves_local_and_external_calls() {
        let graph = build_rust_supergraph(&project(&[
            (
                "src/lib.rs",
                r#"
mod util;
use crate::util::helper;
use std::collections::HashMap;

pub struct Counter { count: u32 }

impl Counter {
    pub fn new() -> Self { Counter { count: 0 } }
    pub fn bump(&mut self) { self.reset(); }
    fn reset(&mut self) { self.count = 0; }
}

pub fn run() {
    let counter = Counter::new();
    helper();
    let map = HashMap::new();
    println!("done");
    missing();
    util::helper();
}
"#,
            ),
            ("src/util.rs", "pub fn helper() {}
"),
        ]));
        let view = CallGraphView::new(&graph);

        let run_targets = view.call_targets_from_caller(callable_id_from_text("src.run"));
        assert!(run_targets.contains(&callable_id_from_text("src.Counter.new")), "{run_targets:?}");
        assert!(run_targets.contains(&callable_id_from_text("src.util.helper")), "{run_targets:?}");
        assert_eq!(
            view.call_targets_from_caller(callable_id_from_text("src.Counter.bump")),
            vec![callable_id_from_text("src.Counter.reset")]
        );
        assert_eq!(graph.language, "rust");
    }
}
