use crate::intern::Sym;
use std::collections::BTreeSet;

use crate::analysis::enrich_supergraph_with_semantic_flows;
use crate::ast::{CallAst, CallContext, ProjectAst, SymbolAst, SymbolKind};
use crate::supergraph::{
    BindingTarget, CallEdgeKind, CallableKind, Confidence, DispatchKind, ExternalTarget,
    ExternalTargetKind, ProgramSupergraph, stable_id,
};
use crate::id_parts;
use crate::supergraph::ids::{NodeId, Tag};

use super::adapter::{
    GraphContext, ImportBinding, LanguageSupergraphAdapter, PendingCall, ResolvedCall,
};
use super::build_initial_supergraph;

const ANALYSIS_VERSION: &str = "typescript-tree-sitter.v1";

pub(crate) fn build_typescript_supergraph(project: &ProjectAst) -> ProgramSupergraph {
    enrich_supergraph_with_semantic_flows(
        build_initial_supergraph(project, TypeScriptSupergraphAdapter),
        project,
    )
}

#[derive(Debug, Clone, Copy)]
struct TypeScriptSupergraphAdapter;

impl TypeScriptSupergraphAdapter {
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
            BindingTarget::Class(class_name) => {
                if let Some(class) = context.classes.get(class_name.as_str()) {
                    ResolvedCall::LocalTarget {
                        callee: class
                            .explicit_constructor_id
                            .clone()
                            .unwrap_or_else(|| class.implicit_constructor_id.clone()),
                        kind: CallEdgeKind::Constructor,
                        evidence: vec![format!("imported class constructor: {class_name}")],
                    }
                } else {
                    ResolvedCall::UnresolvedTarget {
                        target: expression.to_string(),
                        evidence: vec![format!("missing class for import {class_name}")],
                    }
                }
            }
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
            BindingTarget::Module(module) => ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some(module),
                    expression,
                    None,
                    ExternalTargetKind::Unknown,
                    "module_callable",
                    Confidence::Unknown,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Unknown,
                evidence: vec![format!("module object called directly: {module}")],
            },
            _ => ResolvedCall::UnresolvedTarget {
                target: expression.to_string(),
                evidence: vec![format!("unsupported import binding for {expression}")],
            },
        }
    }

    fn resolve_attribute_call(
        &self,
        pending: &PendingCall,
        receiver: &str,
        edge_kind: CallEdgeKind,
        context: &GraphContext,
    ) -> ResolvedCall {
        let expression = pending.call.callee.as_str();
        let member = expression.rsplit('.').next().unwrap_or(expression);

        if receiver == "this" {
            if let Some(class_name) = &pending.class_qualified_name {
                if let Some(class) = context.classes.get(class_name.as_str()) {
                    if let Some(callee) = class.methods.get(member) {
                        return ResolvedCall::LocalTarget {
                            callee: callee.clone(),
                            kind: CallEdgeKind::Method,
                            evidence: vec![format!("this method on {class_name}")],
                        };
                    }
                }
            }
        }

        if let Some(file_context) = context.file_context_for_module(&pending.module_path) {
            if let Some(instance_class) = file_context.assignments.get(receiver) {
                if let Some(class) = context.classes.get(instance_class) {
                    if let Some(callee) = class.methods.get(member) {
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

            if let Some(import) = file_context.imports.get(receiver) {
                return match &import.target {
                    BindingTarget::Module(module) => {
                        let qualified = format!("{module}.{member}");
                        if let Some(callee) = context.local_callables.get(&qualified) {
                            ResolvedCall::LocalTarget {
                                callee: callee.clone(),
                                kind: edge_kind,
                                evidence: vec![format!("imported module member: {qualified}")],
                            }
                        } else {
                            ResolvedCall::ExternalTarget {
                                target: self.external_target(
                                    Some(module),
                                    &qualified,
                                    Some(member),
                                    self.external_kind_for(edge_kind),
                                    "import",
                                    Confidence::Probable,
                                ),
                                kind: CallEdgeKind::External,
                                confidence: Confidence::Probable,
                                evidence: vec![format!(
                                    "external or unresolved module member: {qualified}"
                                )],
                            }
                        }
                    }
                    BindingTarget::External(external) => ResolvedCall::ExternalTarget {
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
                        evidence: vec![format!("external import: {receiver} -> {external}")],
                    },
                    _ => ResolvedCall::UnresolvedTarget {
                        target: expression.to_string(),
                        evidence: vec![format!(
                            "unsupported attribute receiver binding: {receiver}"
                        )],
                    },
                };
            }
        }

        if receiver == "Math" {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some("javascript.Math"),
                    expression,
                    Some(member),
                    ExternalTargetKind::Function,
                    "runtime",
                    Confidence::Exact,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Exact,
                evidence: vec![format!("JavaScript runtime member: {expression}")],
            };
        }

        if matches!(member, "map" | "filter" | "reduce" | "forEach" | "find") {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some("javascript.Array"),
                    &format!("Array.prototype.{member}"),
                    Some(member),
                    ExternalTargetKind::Method,
                    "runtime",
                    Confidence::Probable,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Probable,
                evidence: vec![format!("probable JavaScript array method: {expression}")],
            };
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no receiver binding found for {receiver}")],
        }
    }
}

impl LanguageSupergraphAdapter for TypeScriptSupergraphAdapter {
    fn language(&self) -> &'static str {
        "typescript"
    }

    fn analysis_version(&self) -> &'static str {
        ANALYSIS_VERSION
    }

    fn module_path(&self, _project: &ProjectAst, relative_path: &str) -> String {
        let without_extension = relative_path
            .trim_end_matches(".tsx")
            .trim_end_matches(".ts")
            .trim_end_matches("/index");
        without_extension.replace('/', ".")
    }

    fn normalize_import_module(
        &self,
        current_module: &str,
        import_module: &str,
        local_modules: &BTreeSet<String>,
    ) -> String {
        if !import_module.starts_with('.') {
            return import_module.to_string();
        }

        let mut parts = current_module
            .split('.')
            .map(str::to_string)
            .collect::<Vec<_>>();
        parts.pop();

        for part in import_module.split('/') {
            match part {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other.to_string()),
            }
        }

        let candidate = parts.join(".");
        if local_modules.contains(&candidate) {
            return candidate;
        }
        let index_candidate = if candidate.is_empty() {
            "index".to_string()
        } else {
            format!("{candidate}.index")
        };
        if local_modules.contains(&index_candidate) {
            return index_candidate;
        }
        candidate
    }

    fn implicit_constructor_id(&self, class_name: &str) -> String {
        format!("{class_name}.constructor::<implicit>")
    }

    fn constructor_method_name(&self) -> Option<&'static str> {
        Some("constructor")
    }

    fn callable_kind(&self, symbol: &SymbolAst) -> CallableKind {
        match symbol.kind {
            SymbolKind::Function => CallableKind::Function,
            SymbolKind::Method if symbol.name == "constructor" => CallableKind::Constructor,
            SymbolKind::Method => CallableKind::Method,
            SymbolKind::Class => CallableKind::UnknownCallable,
        }
    }

    fn dispatch_kind(&self, call: &CallAst) -> DispatchKind {
        if call
            .callee
            .chars()
            .next()
            .is_some_and(|first| first.is_uppercase())
        {
            DispatchKind::Direct
        } else {
            match call.context {
                CallContext::Decorator => DispatchKind::Decorator,
                _ if call.receiver.is_some() => DispatchKind::Method,
                _ => DispatchKind::Direct,
            }
        }
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

    fn resolve_call(
        &self,
        pending: &PendingCall,
        _call_site_id: NodeId,
        context: &GraphContext,
    ) -> ResolvedCall {
        let expression = pending.call.callee.as_str();
        let edge_kind = self.edge_kind(&pending.call);
        if let Some(receiver) = &pending.call.receiver {
            return self.resolve_attribute_call(pending, receiver, edge_kind, context);
        }

        if let Some(callee) = context.resolve_direct_local(&pending.module_path, expression) {
            return ResolvedCall::LocalTarget {
                callee,
                kind: edge_kind,
                evidence: vec![format!("direct name lookup: {expression}")],
            };
        }

        if let Some(class) = context.resolve_class_name(&pending.module_path, expression) {
            let callee = class
                .explicit_constructor_id
                .clone()
                .unwrap_or_else(|| class.implicit_constructor_id.clone());
            return ResolvedCall::LocalTarget {
                callee,
                kind: CallEdgeKind::Constructor,
                evidence: vec![format!("class constructor: {}", class.qualified_name)],
            };
        }

        if let Some(file_context) = context.file_context_for_module(&pending.module_path) {
            if let Some(import) = file_context.imports.get(expression) {
                return self.resolve_imported_call(expression, import, edge_kind, context);
            }
        }

        if is_javascript_builtin(expression) {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some("javascript.runtime"),
                    expression,
                    None,
                    ExternalTargetKind::Function,
                    "runtime",
                    Confidence::Exact,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Exact,
                evidence: vec![format!("JavaScript runtime builtin: {expression}")],
            };
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no binding found for {expression}")],
        }
    }
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
        "javascript" => "runtime",
        "react" | "vitest" | "@testing-library" => "npm",
        _ => "unknown",
    }
}

fn is_javascript_builtin(name: &str) -> bool {
    matches!(
        name,
        "Array" | "Boolean" | "Date" | "Map" | "Number" | "Object" | "Promise" | "Set" | "String"
    )
}
