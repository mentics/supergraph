#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{CallAst, FileAst, ProjectAst, SourceSpan, SymbolAst, SymbolKind};
use crate::supergraph::{
    ArgumentShape, Artifact, Binding, BindingKind, BindingTarget, CallEdgeKind, CallSite, Callable,
    CallableKind, Calls, Confidence, Diagnostic, DiagnosticKind, Evidence, EvidenceKind,
    ProgramSupergraph, ProgramSupergraphBuilder, Resolution, Scope, ScopeBindingBehavior,
    ScopeKind, ScopeVariant, Severity, Signature, SourceOwnership, stable_id,
};

mod adapter;
mod python;
mod rust;
mod typescript;

use adapter::{
    ClassInfo, FieldFlowContext, FileContext, GraphContext, ImportBinding,
    LanguageSupergraphAdapter, PendingCall, ResolvedCall,
};

pub fn build_python_supergraph(project: &ProjectAst) -> ProgramSupergraph {
    python::build_python_supergraph(project)
}

pub fn build_rust_supergraph(project: &ProjectAst) -> ProgramSupergraph {
    rust::build_rust_supergraph(project)
}

pub fn build_typescript_supergraph(project: &ProjectAst) -> ProgramSupergraph {
    typescript::build_typescript_supergraph(project)
}

pub fn build_initial_supergraph<A>(project: &ProjectAst, adapter: A) -> ProgramSupergraph
where
    A: LanguageSupergraphAdapter,
{
    let mut lowerer = SourceGraphLowerer::new(project.root.clone(), adapter);
    lowerer.lower_project(project);
    lowerer.finish()
}

#[derive(Debug, Clone)]
pub(crate) struct SourceGraphLowerer<A> {
    adapter: A,
    builder: ProgramSupergraphBuilder,
    file_contexts: BTreeMap<String, FileContext>,
    local_modules: BTreeSet<String>,
    local_callable_ids: BTreeMap<String, String>,
    class_summaries: BTreeMap<String, ClassInfo>,
    short_class_names: BTreeMap<String, Vec<String>>,
    pending_calls: Vec<PendingCall>,
    incoming_local_call_counts: BTreeMap<String, usize>,
}

impl<A> SourceGraphLowerer<A>
where
    A: LanguageSupergraphAdapter,
{
    pub(crate) fn new(root: impl Into<String>, adapter: A) -> Self {
        let root = root.into();
        let language = adapter.language().to_string();
        Self {
            builder: ProgramSupergraphBuilder::new(root, language),
            adapter,
            file_contexts: BTreeMap::new(),
            local_modules: BTreeSet::new(),
            local_callable_ids: BTreeMap::new(),
            class_summaries: BTreeMap::new(),
            short_class_names: BTreeMap::new(),
            pending_calls: Vec::new(),
            incoming_local_call_counts: BTreeMap::new(),
        }
    }

    pub(crate) fn lower_project(&mut self, project: &ProjectAst) {
        for file in &project.files {
            self.local_modules
                .insert(self.adapter.module_path(project, &file.path));
        }

        for file in &project.files {
            self.lower_file(project, file);
        }

        self.populate_imports_and_assignments(project);
        let mut flow_context = FieldFlowContext {
            classes: &mut self.class_summaries,
            files: &self.file_contexts,
            local_modules: &self.local_modules,
            local_callables: &self.local_callable_ids,
            short_classes: &self.short_class_names,
        };
        self.adapter.populate_field_flow(project, &mut flow_context);
        self.resolve_pending_calls();
    }

    pub(crate) fn finish(mut self) -> ProgramSupergraph {
        self.finalize_callable_metadata();
        self.builder.finish()
    }

    pub(crate) fn context(&self) -> SourceGraphContext<'_> {
        SourceGraphContext {
            files: &self.file_contexts,
            local_modules: &self.local_modules,
            local_callable_ids: &self.local_callable_ids,
            class_summaries: &self.class_summaries,
            short_class_names: &self.short_class_names,
            pending_calls: &self.pending_calls,
        }
    }

    fn lower_file(&mut self, project: &ProjectAst, file: &FileAst) {
        let module_path = self.adapter.module_path(project, &file.path);
        let artifact_id = source_id("artifact", &[&file.path]);
        let module_scope_id = source_id("scope", &[&artifact_id, "module"]);
        let module_initializer_id = format!("{module_path}:<module>");
        let module_span = module_span(file);

        self.builder.add_artifact(
            Artifact {
                artifact_id: artifact_id.clone(),
                path: file.path.clone(),
                module_path: module_path.clone(),
                content_hash: None,
            },
            vec![self.source_evidence("source artifact", Some(module_span))],
        );

        self.builder.add_scope(
            Scope {
                scope_id: module_scope_id.clone(),
                parent_scope_id: None,
                artifact_id: artifact_id.clone(),
                kind: ScopeKind::Module,
                variant: ScopeVariant::FileModule,
                language_variant: None,
                binding_behavior: ScopeBindingBehavior::Boundary,
                owner_callable_id: Some(module_initializer_id.clone()),
                span: Some(module_span),
            },
            Confidence::Exact,
            vec![self.source_evidence("module scope", Some(module_span))],
        );

        self.builder.add_callable(
            Callable {
                callable_id: module_initializer_id.clone(),
                kind: CallableKind::ModuleInitializer,
                name: Some("<module>".to_string()),
                qualified_name: format!("{module_path}.<module>"),
                artifact_id: artifact_id.clone(),
                declaration_span: module_span,
                body_span: Some(module_span),
                signature: Signature {
                    parameters: Vec::new(),
                    return_annotation: None,
                },
                scope_id: module_scope_id.clone(),
                attributes: Vec::new(),
                incoming_local_call_count: 0,
                external_invocation_metadata: Vec::new(),
            },
            Confidence::Exact,
            vec![self.source_evidence("module initializer", Some(module_span))],
        );

        self.builder.add_contains(
            artifact_id.clone(),
            module_scope_id.clone(),
            owner_for_artifact(&artifact_id),
            vec![self.source_evidence("artifact contains module scope", Some(module_span))],
        );
        self.builder.add_contains(
            module_scope_id.clone(),
            module_initializer_id.clone(),
            owner_for_scope(&artifact_id, &module_scope_id, Some(&module_initializer_id)),
            vec![self.source_evidence("module scope contains initializer", Some(module_span))],
        );

        self.file_contexts.insert(
            file.path.clone(),
            FileContext {
                artifact_id: artifact_id.clone(),
                module_path: module_path.clone(),
                module_scope_id: module_scope_id.clone(),
                imports: BTreeMap::new(),
                assignments: BTreeMap::new(),
                external_assignments: BTreeMap::new(),
            },
        );

        for symbol in &file.symbols {
            match symbol.kind {
                SymbolKind::Class => self.lower_class(file, symbol, &artifact_id, &module_path),
                SymbolKind::Function | SymbolKind::Method => {
                    self.lower_function(file, symbol, &artifact_id, &module_path)
                }
            }
        }

        for call in &file.calls {
            self.pending_calls.push(PendingCall {
                artifact_id: artifact_id.clone(),
                module_path: module_path.clone(),
                caller_callable_id: module_initializer_id.clone(),
                class_qualified_name: None,
                call: call.clone(),
            });
        }
    }

    fn lower_class(
        &mut self,
        file: &FileAst,
        symbol: &SymbolAst,
        artifact_id: &str,
        module_path: &str,
    ) {
        let class_name = format!("{module_path}.{}", symbol.name);
        let class_scope_id = source_id("scope", &[artifact_id, &class_name]);
        let implicit_constructor_id = self.adapter.implicit_constructor_id(&class_name);
        let module_scope_id = self
            .file_contexts
            .get(&file.path)
            .map(|context| context.module_scope_id.clone())
            .unwrap_or_else(|| source_id("scope", &[artifact_id, "module"]));

        self.builder.add_scope(
            Scope {
                scope_id: class_scope_id.clone(),
                parent_scope_id: Some(module_scope_id.clone()),
                artifact_id: artifact_id.to_string(),
                kind: ScopeKind::Class,
                variant: ScopeVariant::ClassBody,
                language_variant: None,
                binding_behavior: ScopeBindingBehavior::Boundary,
                owner_callable_id: None,
                span: Some(symbol.source_span),
            },
            Confidence::Exact,
            vec![self.source_evidence("class scope", Some(symbol.source_span))],
        );
        self.builder.add_contains(
            module_scope_id.clone(),
            class_scope_id.clone(),
            owner_for_scope(artifact_id, &module_scope_id, None),
            vec![self.source_evidence(
                "module scope contains class scope",
                Some(symbol.source_span),
            )],
        );

        self.class_summaries.insert(
            class_name.clone(),
            ClassInfo {
                qualified_name: class_name.clone(),
                scope_id: class_scope_id.clone(),
                explicit_constructor_id: None,
                implicit_constructor_id: implicit_constructor_id.clone(),
                methods: BTreeMap::new(),
                fields: BTreeMap::new(),
            },
        );
        self.short_class_names
            .entry(symbol.name.clone())
            .or_default()
            .push(class_name.clone());

        self.add_binding(
            Binding {
                binding_id: source_id("binding", &[&module_scope_id, &symbol.name]),
                scope_id: module_scope_id,
                name: symbol.name.clone(),
                kind: BindingKind::Class,
                target: BindingTarget::Class(class_name.clone()),
                span: symbol.source_span,
            },
            artifact_id,
            Confidence::Exact,
        );

        self.builder.add_callable(
            Callable {
                callable_id: implicit_constructor_id.clone(),
                kind: CallableKind::Constructor,
                name: Some(
                    self.adapter
                        .constructor_method_name()
                        .unwrap_or("constructor")
                        .to_string(),
                ),
                qualified_name: implicit_constructor_id.clone(),
                artifact_id: artifact_id.to_string(),
                declaration_span: symbol.source_span,
                body_span: symbol.body_span,
                signature: Signature {
                    parameters: Vec::new(),
                    return_annotation: None,
                },
                scope_id: class_scope_id.clone(),
                attributes: vec!["implicit_constructor".to_string()],
                incoming_local_call_count: 0,
                external_invocation_metadata: Vec::new(),
            },
            Confidence::Exact,
            vec![self.source_evidence("implicit constructor", Some(symbol.source_span))],
        );
        self.builder.add_contains(
            class_scope_id.clone(),
            implicit_constructor_id.clone(),
            owner_for_scope(artifact_id, &class_scope_id, Some(&implicit_constructor_id)),
            vec![self.source_evidence(
                "class scope contains implicit constructor",
                Some(symbol.source_span),
            )],
        );
    }

    fn lower_function(
        &mut self,
        file: &FileAst,
        symbol: &SymbolAst,
        artifact_id: &str,
        module_path: &str,
    ) {
        let qualified_name = symbol_qualified_name(module_path, symbol);
        // Rust symbol ids carry an impl disambiguator when several impls define
        // the same method on one type; keep their callables distinct.
        let callable_id = match symbol.id.split_once(crate::parser::rust::IMPL_MARKER) {
            Some((_, disambiguator)) => {
                format!("{qualified_name}{}{disambiguator}", crate::parser::rust::IMPL_MARKER)
            }
            None => qualified_name.clone(),
        };
        let parent_scope_id = symbol
            .parent
            .as_ref()
            .and_then(|parent| self.class_summaries.get(&format!("{module_path}.{parent}")))
            .map(|class| class.scope_id.clone())
            .or_else(|| {
                self.file_contexts
                    .get(&file.path)
                    .map(|context| context.module_scope_id.clone())
            });
        let scope_id = source_id("scope", &[artifact_id, &qualified_name]);
        let scope_span = symbol.body_span.or(Some(symbol.source_span));

        self.builder.add_scope(
            Scope {
                scope_id: scope_id.clone(),
                parent_scope_id: parent_scope_id.clone(),
                artifact_id: artifact_id.to_string(),
                kind: ScopeKind::Function,
                variant: ScopeVariant::FunctionBody,
                language_variant: None,
                binding_behavior: ScopeBindingBehavior::Boundary,
                owner_callable_id: Some(callable_id.clone()),
                span: scope_span,
            },
            Confidence::Exact,
            vec![self.source_evidence("callable scope", scope_span)],
        );

        if let Some(parent_scope_id) = parent_scope_id.as_ref() {
            self.builder.add_contains(
                parent_scope_id.clone(),
                scope_id.clone(),
                owner_for_scope(artifact_id, parent_scope_id, Some(&callable_id)),
                vec![self.source_evidence("parent scope contains callable scope", scope_span)],
            );
        }

        if let Some(parent) = &symbol.parent {
            if let Some(class) = self
                .class_summaries
                .get_mut(&format!("{module_path}.{parent}"))
            {
                class
                    .methods
                    .insert(symbol.name.clone(), callable_id.clone());
                if self.adapter.constructor_method_name() == Some(symbol.name.as_str()) {
                    class.explicit_constructor_id = Some(callable_id.clone());
                }
            }
            if let Some(class_scope_id) = parent_scope_id.as_ref() {
                self.add_binding(
                    Binding {
                        binding_id: source_id("binding", &[class_scope_id, &symbol.name]),
                        scope_id: class_scope_id.clone(),
                        name: symbol.name.clone(),
                        kind: BindingKind::Method,
                        target: BindingTarget::Callable(callable_id.clone()),
                        span: symbol.source_span,
                    },
                    artifact_id,
                    Confidence::Exact,
                );
            }
        } else if let Some(module_scope_id) = parent_scope_id.as_ref() {
            self.add_binding(
                Binding {
                    binding_id: source_id("binding", &[module_scope_id, &symbol.name]),
                    scope_id: module_scope_id.clone(),
                    name: symbol.name.clone(),
                    kind: BindingKind::Function,
                    target: BindingTarget::Callable(callable_id.clone()),
                    span: symbol.source_span,
                },
                artifact_id,
                Confidence::Exact,
            );
        }

        for parameter in &symbol.parameters {
            self.add_binding(
                Binding {
                    binding_id: source_id("binding", &[&scope_id, &parameter.name]),
                    scope_id: scope_id.clone(),
                    name: parameter.name.clone(),
                    kind: BindingKind::Parameter,
                    target: BindingTarget::Value(parameter.text.clone()),
                    span: parameter.source_span,
                },
                artifact_id,
                Confidence::Exact,
            );
        }

        for assignment in &symbol.assignments {
            self.add_binding(
                Binding {
                    binding_id: source_id("binding", &[&scope_id, &assignment.target]),
                    scope_id: scope_id.clone(),
                    name: assignment.target.clone(),
                    kind: BindingKind::Assignment,
                    target: BindingTarget::Value(
                        assignment
                            .value
                            .clone()
                            .unwrap_or_else(|| assignment.text.clone()),
                    ),
                    span: assignment.source_span,
                },
                artifact_id,
                Confidence::Probable,
            );
        }

        self.local_callable_ids
            .insert(qualified_name.clone(), callable_id.clone());
        self.builder.add_callable(
            Callable {
                callable_id: callable_id.clone(),
                kind: self.adapter.callable_kind(symbol),
                name: Some(symbol.name.clone()),
                qualified_name,
                artifact_id: artifact_id.to_string(),
                declaration_span: symbol.source_span,
                body_span: symbol.body_span,
                signature: Signature {
                    parameters: symbol
                        .parameters
                        .iter()
                        .map(|parameter| parameter.text.clone())
                        .collect(),
                    return_annotation: symbol.return_type.clone(),
                },
                scope_id: scope_id.clone(),
                attributes: symbol
                    .decorators
                    .iter()
                    .map(|decorator| format!("decorator:{}", decorator.text))
                    .collect(),
                incoming_local_call_count: 0,
                external_invocation_metadata: Vec::new(),
            },
            Confidence::Exact,
            vec![self.source_evidence("callable declaration", Some(symbol.source_span))],
        );
        self.builder.add_contains(
            scope_id.clone(),
            callable_id.clone(),
            owner_for_scope(artifact_id, &scope_id, Some(&callable_id)),
            vec![
                self.source_evidence("callable scope contains callable", Some(symbol.source_span)),
            ],
        );

        for call in &symbol.calls {
            self.pending_calls.push(PendingCall {
                artifact_id: artifact_id.to_string(),
                module_path: module_path.to_string(),
                caller_callable_id: callable_id.clone(),
                class_qualified_name: symbol
                    .parent
                    .as_ref()
                    .map(|parent| format!("{module_path}.{parent}")),
                call: call.clone(),
            });
        }
    }

    fn populate_imports_and_assignments(&mut self, project: &ProjectAst) {
        for file in &project.files {
            let module_path = self.adapter.module_path(project, &file.path);
            for import in &file.imports {
                if import.names.is_empty() {
                    self.add_unresolved_import(&file.path, import.text.clone(), import.source_span);
                    continue;
                }

                let context = self.file_contexts.get(&file.path).cloned();
                let Some(context) = context else {
                    continue;
                };
                for import_name in &import.names {
                    let local_name = import_name.alias.clone().unwrap_or_else(|| {
                        import_name
                            .name
                            .rsplit('.')
                            .next()
                            .unwrap_or("")
                            .to_string()
                    });
                    let import_module = import
                        .module
                        .clone()
                        .unwrap_or_else(|| import_name.name.clone());
                    let normalized_module = self.adapter.normalize_import_module(
                        &module_path,
                        &import_module,
                        &self.local_modules,
                    );
                    let graph_context = self.graph_context();
                    let target = if import.module.is_some() {
                        self.adapter.imported_name_target(
                            &normalized_module,
                            &import_name.name,
                            &graph_context,
                        )
                    } else if self.local_modules.contains(&normalized_module) {
                        BindingTarget::Module(normalized_module.clone())
                    } else {
                        BindingTarget::External(format!("{normalized_module}.{local_name}"))
                    };
                    let binding = ImportBinding {
                        module_path: normalized_module.clone(),
                        imported_name: import.module.as_ref().map(|_| import_name.name.clone()),
                        target: target.clone(),
                    };
                    if let Some(file_context) = self.file_contexts.get_mut(&file.path) {
                        file_context.imports.insert(local_name.clone(), binding);
                    }
                    self.add_binding(
                        Binding {
                            binding_id: source_id(
                                "binding",
                                &[&context.module_scope_id, &local_name],
                            ),
                            scope_id: context.module_scope_id.clone(),
                            name: local_name,
                            kind: BindingKind::Import,
                            target,
                            span: import.source_span,
                        },
                        &context.artifact_id,
                        Confidence::Exact,
                    );
                }
            }

            let assignments = file.assignments.clone();
            for assignment in assignments {
                let graph_context = self.graph_context();
                if let Some(class_name) = self.adapter.assignment_class_target(
                    &module_path,
                    file,
                    &assignment,
                    &graph_context,
                ) {
                    if let Some(file_context) = self.file_contexts.get_mut(&file.path) {
                        file_context
                            .assignments
                            .insert(assignment.target.clone(), class_name);
                    }
                } else if let Some(external_name) =
                    self.adapter
                        .assignment_external_target(file, &assignment, &graph_context)
                {
                    if let Some(file_context) = self.file_contexts.get_mut(&file.path) {
                        file_context
                            .external_assignments
                            .insert(assignment.target.clone(), external_name);
                    }
                }
            }
        }
    }

    fn resolve_pending_calls(&mut self) {
        let pending = std::mem::take(&mut self.pending_calls);
        for pending_call in pending {
            let call_site = self.call_site(&pending_call);
            let call_site_id = call_site.call_site_id.clone();
            self.builder.add_call_site(
                call_site,
                Confidence::Exact,
                vec![self.source_evidence("call expression", Some(pending_call.call.source_span))],
            );
            self.builder.add_contains(
                pending_call.caller_callable_id.clone(),
                call_site_id.clone(),
                owner_for_callable(&pending_call.artifact_id, &pending_call.caller_callable_id),
                vec![
                    self.source_evidence(
                        "call site ownership",
                        Some(pending_call.call.source_span),
                    ),
                ],
            );

            let resolved = {
                let graph_context = self.graph_context();
                self.adapter
                    .resolve_call(&pending_call, &call_site_id, &graph_context)
            };
            match resolved {
                ResolvedCall::LocalTarget {
                    callee,
                    kind,
                    evidence,
                } => self.add_calls(
                    &pending_call,
                    call_site_id,
                    kind,
                    Resolution::Exact,
                    Confidence::Exact,
                    Some(callee),
                    None,
                    None,
                    evidence,
                ),
                ResolvedCall::ExternalTarget {
                    target,
                    kind,
                    confidence,
                    evidence,
                } => {
                    let target_id = target.external_target_id.clone();
                    self.builder.add_external_target(
                        target,
                        confidence,
                        self.evidence_strings(&evidence, Some(pending_call.call.source_span)),
                    );
                    self.add_calls(
                        &pending_call,
                        call_site_id,
                        kind,
                        Resolution::External,
                        confidence,
                        None,
                        Some(target_id),
                        None,
                        evidence,
                    );
                }
                ResolvedCall::UnresolvedTarget { target, evidence } => {
                    self.add_diagnostic(
                        DiagnosticKind::UnresolvedSymbol,
                        Severity::Warning,
                        format!("could not resolve call target {target}"),
                        Some(pending_call.artifact_id.clone()),
                        Some(pending_call.call.source_span),
                        vec![call_site_id.clone()],
                    );
                    self.add_calls(
                        &pending_call,
                        call_site_id,
                        CallEdgeKind::PossibleDynamic,
                        Resolution::Unresolved,
                        Confidence::Unknown,
                        None,
                        None,
                        Some(target),
                        evidence,
                    );
                }
            }
        }
    }

    fn call_site(&self, pending: &PendingCall) -> CallSite {
        CallSite {
            call_site_id: source_id(
                "call-site",
                &[
                    &pending.artifact_id,
                    &pending.caller_callable_id,
                    &pending.call.source_span.start_byte.to_string(),
                    &pending.call.source_span.end_byte.to_string(),
                    &pending.call.callee,
                ],
            ),
            artifact_id: pending.artifact_id.clone(),
            enclosing_callable_id: pending.caller_callable_id.clone(),
            span: pending.call.source_span,
            callee_expression: pending.call.callee.clone(),
            argument_shape: call_site_argument_shape(&pending.call),
            dispatch_kind: self.adapter.dispatch_kind(&pending.call),
            context: pending.call.context,
        }
    }

    fn add_calls(
        &mut self,
        pending: &PendingCall,
        call_site_id: String,
        kind: CallEdgeKind,
        resolution: Resolution,
        confidence: Confidence,
        callee_callable_id: Option<String>,
        external_target_id: Option<String>,
        unresolved_target: Option<String>,
        evidence: Vec<String>,
    ) {
        if let Some(callee) = callee_callable_id.as_ref() {
            *self
                .incoming_local_call_counts
                .entry(callee.clone())
                .or_default() += 1;
        }
        self.builder.add_calls(
            Calls {
                caller_callable_id: pending.caller_callable_id.clone(),
                callee_callable_id,
                external_target_id,
                unresolved_target,
                call_site_id,
                kind,
                resolution,
            },
            owner_for_callable(&pending.artifact_id, &pending.caller_callable_id),
            Some(pending.call.source_span),
            confidence,
            self.evidence_strings(&evidence, Some(pending.call.source_span)),
        );
    }

    fn finalize_callable_metadata(&mut self) {
        let incoming_local_call_counts = self.incoming_local_call_counts.clone();
        self.builder.update_callables(|callable| {
            callable.incoming_local_call_count = incoming_local_call_counts
                .get(&callable.callable_id)
                .copied()
                .unwrap_or_default();
            if callable
                .attributes
                .iter()
                .any(|attribute| attribute.starts_with("decorator:"))
                && !callable
                    .external_invocation_metadata
                    .iter()
                    .any(|metadata| metadata == "decorated_callable")
            {
                callable
                    .external_invocation_metadata
                    .push("decorated_callable".to_string());
            }
        });
    }

    fn add_binding(&mut self, binding: Binding, artifact_id: &str, confidence: Confidence) {
        let binding_id = binding.binding_id.clone();
        let scope_id = binding.scope_id.clone();
        let span = binding.span;
        let target_id = self.binding_target_node_id(&binding.target);
        let resolution = binding_resolution(&binding.target);
        self.builder.add_binding(
            binding,
            confidence,
            vec![self.source_evidence("source binding", Some(span))],
        );
        self.builder.add_binds(
            scope_id.clone(),
            binding_id.clone(),
            owner_for_scope(artifact_id, &scope_id, None),
            vec![self.source_evidence("scope binds source binding", Some(span))],
        );

        if let (Some(target_id), Some(resolution)) = (target_id, resolution) {
            self.builder.add_resolves_to_with_resolution(
                binding_id,
                target_id,
                resolution,
                owner_for_scope(artifact_id, &scope_id, None),
                confidence,
                vec![self.resolver_evidence("binding target", Some(span))],
            );
        }
    }

    fn source_evidence(&self, summary: impl Into<String>, span: Option<SourceSpan>) -> Evidence {
        self.evidence(EvidenceKind::Parser, summary, span)
    }

    fn resolver_evidence(&self, summary: impl Into<String>, span: Option<SourceSpan>) -> Evidence {
        self.evidence(EvidenceKind::Resolver, summary, span)
    }

    fn evidence_strings(&self, evidence: &[String], span: Option<SourceSpan>) -> Vec<Evidence> {
        if evidence.is_empty() {
            return vec![self.resolver_evidence("call resolution", span)];
        }
        evidence
            .iter()
            .map(|summary| self.resolver_evidence(summary, span))
            .collect()
    }

    fn evidence(
        &self,
        kind: EvidenceKind,
        summary: impl Into<String>,
        span: Option<SourceSpan>,
    ) -> Evidence {
        Evidence {
            kind,
            summary: format!("{} ({})", summary.into(), self.adapter.analysis_version()),
            source_id: None,
            source_span: span,
            content_hash: None,
            syntax: None,
        }
    }

    fn add_unresolved_import(&mut self, path: &str, import: String, span: SourceSpan) {
        let artifact_id = self
            .file_contexts
            .get(path)
            .map(|context| context.artifact_id.clone());
        self.add_diagnostic(
            DiagnosticKind::UnresolvedImport,
            Severity::Warning,
            format!("could not parse import statement {import}"),
            artifact_id,
            Some(span),
            Vec::new(),
        );
    }

    fn add_diagnostic(
        &mut self,
        kind: DiagnosticKind,
        severity: Severity,
        message: String,
        artifact_id: Option<String>,
        span: Option<SourceSpan>,
        related: Vec<String>,
    ) {
        self.builder.add_diagnostic(
            Diagnostic {
                diagnostic_id: source_id("diagnostic", &[&message, &format!("{span:?}")]),
                kind,
                severity,
                message,
                artifact_id,
                span,
                related,
            },
            Confidence::Exact,
            vec![self.resolver_evidence("source diagnostic", span)],
        );
    }

    fn binding_target_node_id(&self, target: &BindingTarget) -> Option<String> {
        match target {
            BindingTarget::Callable(target_id) | BindingTarget::External(target_id) => {
                Some(target_id.clone())
            }
            BindingTarget::Module(module_path) => self
                .file_contexts
                .values()
                .find(|context| &context.module_path == module_path)
                .map(|context| context.artifact_id.clone()),
            BindingTarget::Class(class_name) => self
                .class_summaries
                .get(class_name)
                .map(|class| class.scope_id.clone()),
            BindingTarget::Value(_) | BindingTarget::Unresolved(_) => None,
        }
    }

    fn graph_context(&self) -> GraphContext<'_> {
        GraphContext {
            files: &self.file_contexts,
            local_modules: &self.local_modules,
            local_callables: &self.local_callable_ids,
            classes: &self.class_summaries,
            short_classes: &self.short_class_names,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SourceGraphContext<'a> {
    pub files: &'a BTreeMap<String, FileContext>,
    pub local_modules: &'a BTreeSet<String>,
    pub local_callable_ids: &'a BTreeMap<String, String>,
    pub class_summaries: &'a BTreeMap<String, ClassInfo>,
    pub short_class_names: &'a BTreeMap<String, Vec<String>>,
    pub pending_calls: &'a [PendingCall],
}

fn module_span(file: &FileAst) -> SourceSpan {
    file.symbols
        .iter()
        .map(|symbol| symbol.source_span)
        .reduce(join_spans)
        .or_else(|| {
            file.imports
                .iter()
                .map(|import| import.source_span)
                .reduce(join_spans)
        })
        .unwrap_or_default()
}

fn symbol_qualified_name(module_path: &str, symbol: &SymbolAst) -> String {
    match &symbol.parent {
        Some(parent) => format!("{module_path}.{parent}.{}", symbol.name),
        None => format!("{module_path}.{}", symbol.name),
    }
}

fn join_spans(left: SourceSpan, right: SourceSpan) -> SourceSpan {
    SourceSpan {
        start_byte: left.start_byte.min(right.start_byte),
        end_byte: left.end_byte.max(right.end_byte),
        start_row: left.start_row.min(right.start_row),
        start_column: if left.start_row <= right.start_row {
            left.start_column
        } else {
            right.start_column
        },
        end_row: left.end_row.max(right.end_row),
        end_column: if left.end_row >= right.end_row {
            left.end_column
        } else {
            right.end_column
        },
    }
}

fn source_id(prefix: &str, parts: &[&str]) -> String {
    stable_id(prefix, parts)
}

fn owner_for_artifact(artifact_id: &str) -> SourceOwnership {
    SourceOwnership {
        artifact_id: Some(artifact_id.to_string()),
        scope_id: None,
        callable_id: None,
    }
}

fn owner_for_scope(
    artifact_id: &str,
    scope_id: &str,
    callable_id: Option<&str>,
) -> SourceOwnership {
    SourceOwnership {
        artifact_id: Some(artifact_id.to_string()),
        scope_id: Some(scope_id.to_string()),
        callable_id: callable_id.map(str::to_string),
    }
}

fn owner_for_callable(artifact_id: &str, callable_id: &str) -> SourceOwnership {
    SourceOwnership {
        artifact_id: Some(artifact_id.to_string()),
        scope_id: None,
        callable_id: Some(callable_id.to_string()),
    }
}

fn binding_resolution(target: &BindingTarget) -> Option<Resolution> {
    match target {
        BindingTarget::Callable(_) | BindingTarget::Class(_) | BindingTarget::Module(_) => {
            Some(Resolution::Exact)
        }
        BindingTarget::External(_) => Some(Resolution::External),
        BindingTarget::Value(_) | BindingTarget::Unresolved(_) => None,
    }
}

fn call_site_argument_shape(call: &CallAst) -> ArgumentShape {
    ArgumentShape {
        positional_count: call.args_count.saturating_sub(call.argument_names.len()),
        named_arguments: call.argument_names.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{CallContext, DecoratorAst};
    use crate::supergraph::{ExternalTarget, ExternalTargetKind, NodeFact};

    #[derive(Debug, Clone, Copy)]
    struct TestAdapter;

    impl LanguageSupergraphAdapter for TestAdapter {
        fn language(&self) -> &'static str {
            "test"
        }

        fn analysis_version(&self) -> &'static str {
            "test.v1"
        }

        fn module_path(&self, _project: &ProjectAst, relative_path: &str) -> String {
            relative_path
                .strip_suffix(".py")
                .unwrap_or(relative_path)
                .to_string()
        }

        fn normalize_import_module(
            &self,
            _current_module: &str,
            import_module: &str,
            _local_modules: &BTreeSet<String>,
        ) -> String {
            import_module.to_string()
        }

        fn implicit_constructor_id(&self, class_name: &str) -> String {
            format!("{class_name}.__init__")
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
            _confidence: Confidence,
        ) -> ExternalTarget {
            ExternalTarget {
                external_target_id: source_id("external", &[qualified_name]),
                ecosystem: "test".to_string(),
                package_name: None,
                package_version: None,
                module_path: module_path.map(str::to_string),
                qualified_name: qualified_name.to_string(),
                member_path: member_path.map(str::to_string),
                target_kind,
                source: source.to_string(),
            }
        }

        fn resolve_call(
            &self,
            pending: &PendingCall,
            _call_site_id: &str,
            context: &GraphContext,
        ) -> ResolvedCall {
            if let Some(callee) =
                context.resolve_direct_local(&pending.module_path, &pending.call.callee)
            {
                ResolvedCall::LocalTarget {
                    callee,
                    kind: self.edge_kind(&pending.call),
                    evidence: vec![format!("local test call: {}", pending.call.callee)],
                }
            } else {
                ResolvedCall::UnresolvedTarget {
                    target: pending.call.callee.clone(),
                    evidence: vec![format!("unresolved test call: {}", pending.call.callee)],
                }
            }
        }
    }

    #[test]
    fn finalizes_incoming_counts_and_decorated_metadata() {
        let graph = build_initial_supergraph(&metadata_project(), TestAdapter);

        let caller = callable(&graph, "sample.caller");
        assert_eq!(caller.incoming_local_call_count, 0);
        assert!(
            caller
                .external_invocation_metadata
                .iter()
                .any(|metadata| metadata == "decorated_callable"),
            "decorated callable should retain external invocation metadata"
        );

        let callee = callable(&graph, "sample.callee");
        assert_eq!(callee.incoming_local_call_count, 1);
        assert!(
            callee.external_invocation_metadata.is_empty(),
            "undecorated callable should not receive decorated metadata"
        );
    }

    fn metadata_project() -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: "sample.py".to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![
                    symbol("callee", Vec::new(), Vec::new(), span(0, 20)),
                    symbol(
                        "caller",
                        vec![DecoratorAst {
                            text: "route".to_string(),
                            source_span: span(22, 28),
                        }],
                        vec![CallAst {
                            callee: "callee".to_string(),
                            receiver: None,
                            argument_names: Vec::new(),
                            args_count: 0,
                            context: CallContext::Body,
                            source_span: span(40, 48),
                        }],
                        span(30, 60),
                    ),
                ],
                statements: Vec::new(),
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn symbol(
        name: &str,
        decorators: Vec<DecoratorAst>,
        calls: Vec<CallAst>,
        source_span: SourceSpan,
    ) -> SymbolAst {
        SymbolAst {
            id: format!("sample:{name}"),
            name: name.to_string(),
            kind: SymbolKind::Function,
            module_path: "sample".to_string(),
            parent: None,
            parameters: Vec::new(),
            decorators,
            return_type: None,
            body_span: Some(source_span),
            assignments: Vec::new(),
            calls,
            raises: Vec::new(),
            statements: Vec::new(),
            expressions: Vec::new(),
            conditions: Vec::new(),
            definitions: Vec::new(),
            uses: Vec::new(),
            returns: Vec::new(),
            field_accesses: Vec::new(),
            index_accesses: Vec::new(),
            source_span,
        }
    }

    fn callable<'a>(graph: &'a ProgramSupergraph, qualified_name: &str) -> &'a Callable {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Callable(callable) if callable.qualified_name == qualified_name => {
                    Some(callable)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing callable {qualified_name}"))
    }

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start,
            end_byte: end,
            start_row: 0,
            start_column: start,
            end_row: 0,
            end_column: end,
        }
    }
}
