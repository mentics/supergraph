use crate::intern::Sym;
use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{AssignmentAst, CallAst, CallContext, FileAst, ProjectAst, SymbolAst, SymbolKind};
use crate::supergraph::ids::NodeId;
use crate::supergraph::{
    BindingTarget, CallEdgeKind, CallableKind, Confidence, DispatchKind, ExternalTarget,
    ExternalTargetKind,
};

#[derive(Debug, Clone)]
pub(crate) struct ClassInfo {
    pub qualified_name: String,
    pub scope_id: NodeId,
    pub explicit_constructor_id: Option<NodeId>,
    pub implicit_constructor_id: NodeId,
    pub methods: BTreeMap<String, NodeId>,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportBinding {
    pub module_path: String,
    pub imported_name: Option<String>,
    pub target: BindingTarget,
}

#[derive(Debug, Clone)]
pub(crate) struct FileContext {
    pub artifact_id: NodeId,
    pub module_path: String,
    pub module_scope_id: NodeId,
    pub imports: BTreeMap<String, ImportBinding>,
    pub assignments: BTreeMap<String, String>,
    pub external_assignments: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingCall {
    pub artifact_id: NodeId,
    pub module_path: String,
    pub caller_callable_id: NodeId,
    pub class_qualified_name: Option<String>,
    pub call: CallAst,
}

#[derive(Debug, Clone)]
pub(crate) enum ResolvedCall {
    LocalTarget {
        callee: NodeId,
        kind: CallEdgeKind,
        evidence: Vec<String>,
    },
    ExternalTarget {
        target: ExternalTarget,
        kind: CallEdgeKind,
        confidence: Confidence,
        evidence: Vec<String>,
    },
    UnresolvedTarget {
        target: String,
        evidence: Vec<String>,
    },
}

pub(crate) struct GraphContext<'a> {
    pub files: &'a BTreeMap<String, FileContext>,
    pub local_modules: &'a BTreeSet<String>,
    pub local_callables: &'a BTreeMap<String, NodeId>,
    pub classes: &'a BTreeMap<String, ClassInfo>,
    pub short_classes: &'a BTreeMap<String, Vec<String>>,
}

impl GraphContext<'_> {
    pub fn file_context_for_module(&self, module_path: &str) -> Option<&FileContext> {
        self.files
            .values()
            .find(|context| context.module_path == module_path)
    }

    pub fn resolve_direct_local(&self, module_path: &str, name: &str) -> Option<NodeId> {
        self.local_callables
            .get(&format!("{module_path}.{name}"))
            .copied()
    }

    pub fn resolve_class_name(&self, module_path: &str, name: &str) -> Option<&ClassInfo> {
        self.classes
            .get(&format!("{module_path}.{name}"))
            .or_else(|| {
                self.short_classes
                    .get(name)
                    .and_then(|classes| classes.first())
                    .and_then(|class_name| self.classes.get(class_name))
            })
    }
}

pub(crate) trait LanguageSupergraphAdapter {
    fn language(&self) -> &'static str;

    fn analysis_version(&self) -> &'static str;

    fn module_path(&self, project: &ProjectAst, relative_path: &str) -> String;

    fn normalize_import_module(
        &self,
        current_module: &str,
        import_module: &str,
        local_modules: &BTreeSet<String>,
    ) -> String;

    fn implicit_constructor_id(&self, class_name: &str) -> String;

    fn constructor_method_name(&self) -> Option<&'static str>;

    fn callable_kind(&self, symbol: &SymbolAst) -> CallableKind {
        match symbol.kind {
            SymbolKind::Function => CallableKind::Function,
            SymbolKind::Method if self.constructor_method_name() == Some(symbol.name.as_str()) => {
                CallableKind::Constructor
            }
            SymbolKind::Method => CallableKind::Method,
            SymbolKind::Class => CallableKind::UnknownCallable,
        }
    }

    fn dispatch_kind(&self, call: &CallAst) -> DispatchKind {
        match call.context {
            CallContext::Decorator => DispatchKind::Decorator,
            _ if call.receiver.is_some() => DispatchKind::Method,
            _ => DispatchKind::Direct,
        }
    }

    fn edge_kind(&self, call: &CallAst) -> CallEdgeKind {
        match call.context {
            CallContext::Decorator => CallEdgeKind::Decorator,
            _ if call.receiver.is_some() => CallEdgeKind::Method,
            _ => CallEdgeKind::Direct,
        }
    }

    fn external_kind_for(&self, kind: CallEdgeKind) -> ExternalTargetKind {
        match kind {
            CallEdgeKind::Constructor => ExternalTargetKind::Constructor,
            CallEdgeKind::Decorator => ExternalTargetKind::Decorator,
            CallEdgeKind::Method => ExternalTargetKind::Method,
            CallEdgeKind::Direct => ExternalTargetKind::Function,
            CallEdgeKind::External | CallEdgeKind::PossibleDynamic => ExternalTargetKind::Unknown,
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
    ) -> ExternalTarget;

    fn imported_name_target(
        &self,
        module: &str,
        name: &str,
        context: &GraphContext,
    ) -> BindingTarget {
        let qualified = format!("{module}.{name}");
        if let Some(callable) = context.local_callables.get(&qualified) {
            BindingTarget::Callable(*callable)
        } else if context.classes.contains_key(&qualified) {
            BindingTarget::Class(Sym::from(qualified))
        } else if context.local_modules.contains(module) {
            BindingTarget::Unresolved(Sym::from(qualified))
        } else {
            BindingTarget::External(Sym::from(qualified))
        }
    }

    fn assignment_class_target(
        &self,
        module_path: &str,
        file: &FileAst,
        assignment: &AssignmentAst,
        context: &GraphContext,
    ) -> Option<String> {
        let callee = assignment.value.as_deref().and_then(call_expression_name)?;
        context
            .resolve_class_name(module_path, callee)
            .map(|class| class.qualified_name.clone())
            .or_else(|| {
                (context
                    .files
                    .get(&file.path)
                    .and_then(|file_context| file_context.imports.get(callee))
                    .and_then(|import| match &import.target {
                        BindingTarget::Class(class_name) => Some(class_name.clone()),
                        _ => None,
                    })).map(|sym| sym.to_string())
            })
    }

    fn assignment_external_target(
        &self,
        file: &FileAst,
        assignment: &AssignmentAst,
        context: &GraphContext,
    ) -> Option<String> {
        let callee = assignment.value.as_deref().and_then(call_expression_name)?;
        (context
            .files
            .get(&file.path)
            .and_then(|file_context| file_context.imports.get(callee))
            .and_then(|import| match &import.target {
                BindingTarget::External(external) => Some(external.clone()),
                _ => None,
            })).map(|sym| sym.to_string())
    }

    fn populate_field_flow(&self, _project: &ProjectAst, _context: &mut FieldFlowContext<'_>) {}

    fn resolve_call(
        &self,
        pending: &PendingCall,
        call_site_id: NodeId,
        context: &GraphContext,
    ) -> ResolvedCall;
}

pub(crate) struct FieldFlowContext<'a> {
    pub classes: &'a mut BTreeMap<String, ClassInfo>,
    pub files: &'a BTreeMap<String, FileContext>,
    pub local_modules: &'a BTreeSet<String>,
    pub local_callables: &'a BTreeMap<String, NodeId>,
    pub short_classes: &'a BTreeMap<String, Vec<String>>,
}

pub(crate) fn call_expression_name(value: &str) -> Option<&str> {
    value.split_once('(').map(|(callee, _)| callee.trim())
}
