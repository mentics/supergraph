use std::collections::BTreeSet;
use std::path::Path;

use crate::analysis::enrich_supergraph_with_semantic_flows;
use crate::ast::ProjectAst;
use crate::supergraph::{
    BindingTarget, CallEdgeKind, Confidence, ExternalTarget, ExternalTargetKind, ProgramSupergraph,
    stable_id,
};

use super::adapter::{
    FieldFlowContext, GraphContext, ImportBinding, LanguageSupergraphAdapter, PendingCall,
    ResolvedCall,
};
use super::build_initial_supergraph;

const ANALYSIS_VERSION: &str = "python-tree-sitter.v1";

pub(crate) fn build_python_supergraph(project: &ProjectAst) -> ProgramSupergraph {
    let g = build_initial_supergraph(project, PythonSupergraphAdapter::new(project));
    enrich_supergraph_with_semantic_flows(g, project)
}

#[derive(Debug, Clone)]
struct PythonSupergraphAdapter {
    package: Option<String>,
}

impl PythonSupergraphAdapter {
    fn new(project: &ProjectAst) -> Self {
        Self {
            package: package_name(&project.root),
        }
    }

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
                if let Some(class) = context.classes.get(class_name) {
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

        if receiver == "self" {
            if let Some(class_name) = &pending.class_qualified_name {
                if let Some(class) = context.classes.get(class_name) {
                    if let Some(callee) = class.methods.get(member) {
                        return ResolvedCall::LocalTarget {
                            callee: callee.clone(),
                            kind: CallEdgeKind::Method,
                            evidence: vec![format!("self method on {class_name}")],
                        };
                    }
                }
            }
        }

        if let Some(field) = receiver.strip_prefix("self.") {
            if let Some(class_name) = &pending.class_qualified_name {
                if let Some(owner_class) = context.classes.get(class_name) {
                    if let Some(field_class_name) = owner_class.fields.get(field) {
                        if let Some(field_class) = context.classes.get(field_class_name) {
                            if let Some(callee) = field_class.methods.get(member) {
                                return ResolvedCall::LocalTarget {
                                    callee: callee.clone(),
                                    kind: CallEdgeKind::Method,
                                    evidence: vec![format!(
                                        "constructor field flow: {class_name}.{field} -> {field_class_name}"
                                    )],
                                };
                            }
                        }
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

            if let Some(external_class) = file_context.external_assignments.get(receiver) {
                return ResolvedCall::ExternalTarget {
                    target: self.external_target(
                        Some(external_class),
                        &format!("{external_class}.{member}"),
                        Some(member),
                        self.external_kind_for(edge_kind),
                        "assignment",
                        Confidence::Probable,
                    ),
                    kind: CallEdgeKind::External,
                    confidence: Confidence::Probable,
                    evidence: vec![format!(
                        "external instance assignment: {receiver} -> {external_class}"
                    )],
                };
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
                            ResolvedCall::UnresolvedTarget {
                                target: expression.to_string(),
                                evidence: vec![format!("module {module} has no callable {member}")],
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
                    BindingTarget::Class(class_name) => {
                        if let Some(class) = context.classes.get(class_name) {
                            if let Some(callee) = class.methods.get(member) {
                                ResolvedCall::LocalTarget {
                                    callee: callee.clone(),
                                    kind: CallEdgeKind::Method,
                                    evidence: vec![format!("class member: {class_name}.{member}")],
                                }
                            } else {
                                ResolvedCall::UnresolvedTarget {
                                    target: expression.to_string(),
                                    evidence: vec![format!(
                                        "class {class_name} has no method {member}"
                                    )],
                                }
                            }
                        } else {
                            ResolvedCall::UnresolvedTarget {
                                target: expression.to_string(),
                                evidence: vec![format!("unknown class binding {class_name}")],
                            }
                        }
                    }
                    _ => ResolvedCall::UnresolvedTarget {
                        target: expression.to_string(),
                        evidence: vec![format!(
                            "unsupported attribute receiver binding: {receiver}"
                        )],
                    },
                };
            }
        }

        if receiver.contains('.') {
            return ResolvedCall::UnresolvedTarget {
                target: expression.to_string(),
                evidence: vec![format!("dynamic receiver chain: {receiver}")],
            };
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no receiver binding found for {receiver}")],
        }
    }

    fn resolve_type_name(
        &self,
        file_path: &str,
        module_path: &str,
        type_name: &str,
        context: &GraphContext,
    ) -> Option<String> {
        context
            .resolve_class_name(module_path, type_name)
            .map(|class| class.qualified_name.clone())
            .or_else(|| {
                context
                    .files
                    .get(file_path)
                    .and_then(|file_context| file_context.imports.get(type_name))
                    .and_then(|import| match &import.target {
                        BindingTarget::Class(class_name) => Some(class_name.clone()),
                        _ => None,
                    })
            })
    }
}

impl LanguageSupergraphAdapter for PythonSupergraphAdapter {
    fn language(&self) -> &'static str {
        "python"
    }

    fn analysis_version(&self) -> &'static str {
        ANALYSIS_VERSION
    }

    fn module_path(&self, _project: &ProjectAst, relative_path: &str) -> String {
        let local = relative_path
            .trim_end_matches(".py")
            .trim_end_matches("/__init__")
            .replace('/', ".");
        match (&self.package, local.is_empty()) {
            (Some(package), true) => package.clone(),
            (Some(package), false) => format!("{package}.{local}"),
            (None, _) => local,
        }
    }

    fn normalize_import_module(
        &self,
        _current_module: &str,
        import_module: &str,
        local_modules: &BTreeSet<String>,
    ) -> String {
        if local_modules.contains(import_module) {
            return import_module.to_string();
        }
        if let Some(package) = &self.package {
            let prefixed = format!("{package}.");
            if import_module.starts_with(&prefixed) {
                return import_module.to_string();
            }
            let packaged = format!("{package}.{import_module}");
            if local_modules.contains(&packaged) {
                return packaged;
            }
        }
        import_module.to_string()
    }

    fn implicit_constructor_id(&self, class_name: &str) -> String {
        format!("{class_name}.__init__::<implicit>")
    }

    fn constructor_method_name(&self) -> Option<&'static str> {
        Some("__init__")
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

    fn populate_field_flow(&self, project: &ProjectAst, flow_context: &mut FieldFlowContext<'_>) {
        let field_updates = {
            let graph_context = GraphContext {
                files: flow_context.files,
                local_modules: flow_context.local_modules,
                local_callables: flow_context.local_callables,
                classes: flow_context.classes,
                short_classes: flow_context.short_classes,
            };
            let mut updates = Vec::new();

            for file in &project.files {
                let module_path = self.module_path(project, &file.path);
                for symbol in &file.symbols {
                    if symbol.name != "__init__" {
                        continue;
                    }
                    let Some(parent) = &symbol.parent else {
                        continue;
                    };
                    let class_name = format!("{module_path}.{parent}");
                    let parameter_types = symbol
                        .parameters
                        .iter()
                        .filter_map(|parameter| {
                            parameter_type(&parameter.text).and_then(|type_name| {
                                self.resolve_type_name(
                                    &file.path,
                                    &module_path,
                                    &type_name,
                                    &graph_context,
                                )
                                .map(|qualified| (parameter.name.clone(), qualified))
                            })
                        })
                        .collect::<std::collections::BTreeMap<_, _>>();

                    for assignment in &symbol.assignments {
                        let Some(field) = assignment.target.strip_prefix("self.") else {
                            continue;
                        };
                        let Some(value) = assignment.value.as_ref() else {
                            continue;
                        };
                        if let Some(class_target) = parameter_types.get(value.trim()) {
                            updates.push((
                                class_name.clone(),
                                field.to_string(),
                                class_target.clone(),
                            ));
                        }
                    }
                }
            }
            updates
        };

        for (class_name, field, class_target) in field_updates {
            if let Some(class) = flow_context.classes.get_mut(&class_name) {
                class.fields.insert(field, class_target);
            }
        }
    }

    fn resolve_call(
        &self,
        pending: &PendingCall,
        _call_site_id: &str,
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

        if is_builtin_callable(expression) {
            return ResolvedCall::ExternalTarget {
                target: self.external_target(
                    Some("python.runtime"),
                    expression,
                    None,
                    ExternalTargetKind::Function,
                    "runtime",
                    Confidence::Exact,
                ),
                kind: CallEdgeKind::External,
                confidence: Confidence::Exact,
                evidence: vec![format!("python runtime builtin: {expression}")],
            };
        }

        ResolvedCall::UnresolvedTarget {
            target: expression.to_string(),
            evidence: vec![format!("no binding found for {expression}")],
        }
    }
}

fn package_name(root: &str) -> Option<String> {
    Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
}

fn parameter_type(parameter_text: &str) -> Option<String> {
    let (_, annotation) = parameter_text.split_once(':')?;
    Some(
        annotation
            .split('=')
            .next()
            .unwrap_or(annotation)
            .split('|')
            .next()
            .unwrap_or(annotation)
            .trim()
            .to_string(),
    )
    .filter(|annotation| !annotation.is_empty())
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
        external_target_id: stable_id(
            "external",
            &[
                module_path.as_deref().unwrap_or("unknown"),
                qualified_name,
                member_path.unwrap_or(""),
                &format!("{target_kind:?}"),
            ],
        ),
        ecosystem: package_name
            .as_deref()
            .map(ecosystem_for_package)
            .unwrap_or("unknown")
            .to_string(),
        package_name,
        package_version: None,
        module_path,
        qualified_name: qualified_name.to_string(),
        member_path: member_path.map(str::to_string),
        target_kind,
        source: source.to_string(),
    }
}

fn ecosystem_for_package(package: &str) -> &'static str {
    match package {
        "datetime" | "uuid" | "typing" | "python" => "stdlib",
        "fastapi" | "pydantic" => "pypi",
        _ => "unknown",
    }
}

fn is_builtin_callable(name: &str) -> bool {
    matches!(
        name,
        "dict" | "list" | "set" | "tuple" | "len" | "str" | "int" | "bool"
    )
}
