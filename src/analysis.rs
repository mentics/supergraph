use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::{
    ast::{CallAst, ProjectAst, SourceSpan, SymbolAst, SymbolKind},
    fs::{
        PythonSourceFile, RustSourceFile, TypeScriptSourceFile, discover_python_files,
        discover_rust_files, discover_typescript_files,
    },
    parser::{
        python::{filter_file as filter_python_file, parse_python_file},
        rust::{filter_file as filter_rust_file, parse_rust_file},
        typescript::{filter_file as filter_typescript_file, parse_typescript_file},
    },
    timing,
    supergraph::{
        self as sg, Confidence, EdgeFact, EdgeKind, Evidence, EvidenceKind, GraphEdge, GraphNode,
        NodeFact, NodeId, NodeKind, ProgramSupergraph, SourceOwnership, build_indexes,
        refresh_fact_identity, refresh_provenance, refresh_uncertainty, sort_graph, stable_id,
        uncertainty_from_confidence,
    },
};
use source_graph::{build_python_supergraph, build_rust_supergraph, build_typescript_supergraph};

mod callable_index;
pub mod calls;
pub mod cfg;
pub mod control_dependence;
pub mod data_flow;
pub mod expressions;
pub mod interprocedural;
pub mod scopes;
pub mod source_graph;
pub mod statements;
pub mod structure;
pub mod symbols;
pub mod values;

/// Applies `work` to every item on scoped threads and returns the results in input order.
/// The first error in input order is returned, matching a sequential loop.
fn parallel_map<T: Sync, R: Send>(
    items: &[T],
    work: impl Fn(&T) -> Result<R> + Sync,
) -> Result<Vec<R>> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk_size = items.len().div_ceil(threads).max(1);
    let work = &work;
    let chunks = std::thread::scope(|scope| {
        let handles = items
            .chunks(chunk_size)
            .map(|chunk| scope.spawn(move || chunk.iter().map(work).collect::<Result<Vec<R>>>()))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("parse worker panicked"))
            .collect::<Vec<_>>()
    });
    let mut results = Vec::with_capacity(items.len());
    for chunk in chunks {
        results.extend(chunk?);
    }
    Ok(results)
}

pub fn analyze_python_path(path: impl AsRef<Path>) -> Result<ProjectAst> {
    let path = path.as_ref();
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize analysis root {}", path.display()))?;
    let files = if root.is_file() {
        vec![PythonSourceFile {
            absolute_path: root.clone(),
            relative_path: root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string()),
        }]
    } else {
        timing::stage("discover files", || discover_python_files(&root))?
    };

    let parsed_files = timing::stage("parse + extract AST (parallel)", || {
        parallel_map(&files, |file| {
            let parsed = timing::add_cpu("  tree-sitter parse", || {
                parse_python_file(&file.absolute_path, file.relative_path.clone())
            })
            .with_context(|| {
                format!("failed to parse Python source {}", file.absolute_path.display())
            })?;
            Ok(timing::add_cpu("  AST extraction", || filter_python_file(&parsed)))
        })
    })?;

    Ok(ProjectAst {
        root: analysis_root(&root).display().to_string(),
        files: parsed_files,
    })
}

pub fn analyze_python_supergraph(path: impl AsRef<Path>) -> Result<ProgramSupergraph> {
    analyze_python_path(path).map(|project| build_python_supergraph(&project))
}

pub fn analyze_rust_path(path: impl AsRef<Path>) -> Result<ProjectAst> {
    let path = path.as_ref();
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize analysis root {}", path.display()))?;
    let files = if root.is_file() {
        vec![RustSourceFile {
            absolute_path: root.clone(),
            relative_path: root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string()),
        }]
    } else {
        timing::stage("discover files", || discover_rust_files(&root))?
    };

    let parsed_files = timing::stage("parse + extract AST (parallel)", || {
        parallel_map(&files, |file| {
            let parsed = timing::add_cpu("  tree-sitter parse", || {
                parse_rust_file(&file.absolute_path, file.relative_path.clone())
            })
            .with_context(|| {
                format!("failed to parse Rust source {}", file.absolute_path.display())
            })?;
            Ok(timing::add_cpu("  AST extraction", || filter_rust_file(&parsed)))
        })
    })?;

    Ok(ProjectAst {
        root: analysis_root(&root).display().to_string(),
        files: parsed_files,
    })
}

pub fn analyze_rust_supergraph(path: impl AsRef<Path>) -> Result<ProgramSupergraph> {
    analyze_rust_path(path).map(|project| build_rust_supergraph(&project))
}

pub fn analyze_typescript_path(path: impl AsRef<Path>) -> Result<ProjectAst> {
    let path = path.as_ref();
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize analysis root {}", path.display()))?;
    let files = if root.is_file() {
        vec![TypeScriptSourceFile {
            absolute_path: root.clone(),
            relative_path: root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string()),
        }]
    } else {
        timing::stage("discover files", || discover_typescript_files(&root))?
    };

    let parsed_files = timing::stage("parse + extract AST (parallel)", || {
        parallel_map(&files, |file| {
            let parsed = timing::add_cpu("  tree-sitter parse", || {
                parse_typescript_file(&file.absolute_path, file.relative_path.clone())
            })
            .with_context(|| {
                format!("failed to parse TypeScript source {}", file.absolute_path.display())
            })?;
            Ok(timing::add_cpu("  AST extraction", || filter_typescript_file(&parsed)))
        })
    })?;

    Ok(ProjectAst {
        root: analysis_root(&root).display().to_string(),
        files: parsed_files,
    })
}

pub fn analyze_typescript_supergraph(path: impl AsRef<Path>) -> Result<ProgramSupergraph> {
    analyze_typescript_path(path).map(|project| build_typescript_supergraph(&project))
}

fn analysis_root(root: &Path) -> PathBuf {
    if root.is_file() {
        root.parent().unwrap_or(root).to_path_buf()
    } else {
        root.to_path_buf()
    }
}

pub fn enrich_supergraph_with_semantic_flows(
    mut graph: ProgramSupergraph,
    project: &ProjectAst,
) -> ProgramSupergraph {
    let context = timing::stage("semantic context", || SemanticContext::new(&graph, project));
    timing::stage("pass: statements", || statements::emit(&mut graph, &context));
    timing::stage("pass: expressions", || expressions::emit(&mut graph, &context));
    timing::stage("pass: structure", || structure::emit(&mut graph, &context));
    timing::stage("pass: scopes", || scopes::emit(&mut graph, &context));
    timing::stage("pass: symbols", || symbols::emit(&mut graph, &context));
    timing::stage("pass: calls", || calls::emit(&mut graph));
    timing::stage("pass: values", || values::emit(&mut graph, &context));
    timing::stage("pass: cfg", || cfg::emit(&mut graph, &context));
    timing::stage("pass: data flow", || data_flow::emit(&mut graph, &context));
    timing::stage("pass: control dependence", || control_dependence::emit(&mut graph, &context));
    timing::stage("pass: interprocedural", || interprocedural::emit(&mut graph));
    timing::stage("pass: refresh uncertainty", || refresh_uncertainty(&mut graph));
    timing::stage("pass: refresh provenance", || refresh_provenance(&mut graph));
    timing::stage("pass: fact identity hashes", || refresh_fact_identity(&mut graph));
    timing::stage("pass: sort graph", || sort_graph(&mut graph));
    graph.indexes = timing::stage("build indexes", || build_indexes(&graph.nodes, &graph.edges));
    graph
}

pub(crate) struct SemanticContext<'a> {
    project: &'a ProjectAst,
    artifacts_by_path: BTreeMap<String, sg::Artifact>,
    callables_by_owner: BTreeMap<String, sg::Callable>,
    call_sites_by_key: BTreeMap<String, sg::CallSite>,
}

impl<'a> SemanticContext<'a> {
    fn new(graph: &ProgramSupergraph, project: &'a ProjectAst) -> Self {
        let mut artifacts_by_path = BTreeMap::new();
        let mut artifacts_by_module = BTreeMap::new();
        let mut callables = Vec::new();
        let mut call_sites_by_key = BTreeMap::new();

        for node in &graph.nodes {
            match &node.fact {
                NodeFact::Artifact(artifact) => {
                    artifacts_by_module.insert(artifact.module_path.clone(), artifact.clone());
                    artifacts_by_path.insert(artifact.path.clone(), artifact.clone());
                }
                NodeFact::Callable(callable) => callables.push(callable.clone()),
                NodeFact::CallSite(call_site) => {
                    call_sites_by_key.insert(call_site_key(call_site), call_site.clone());
                }
                _ => {}
            }
        }

        let mut callables_by_owner = BTreeMap::new();
        for callable in callables {
            if callable.kind == sg::CallableKind::ModuleInitializer {
                callables_by_owner.insert(callable.callable_id.clone(), callable.clone());
            }
            for artifact in artifacts_by_module.values() {
                let module_path = &artifact.module_path;
                if callable.kind == sg::CallableKind::ModuleInitializer {
                    callables_by_owner.insert(
                        format!("{}:<module>", local_module_path(&artifact.path)),
                        callable.clone(),
                    );
                }
                let prefix = format!("{module_path}.");
                if let Some(local_name) = callable.qualified_name.strip_prefix(&prefix) {
                    callables_by_owner
                        .insert(format!("{module_path}:{local_name}"), callable.clone());
                    callables_by_owner.insert(
                        format!("{}:{local_name}", local_module_path(&artifact.path)),
                        callable.clone(),
                    );
                }
            }
        }

        Self {
            project,
            artifacts_by_path,
            callables_by_owner,
            call_sites_by_key,
        }
    }

    pub(crate) fn semantic_callables(&'a self) -> Vec<SemanticCallable<'a>> {
        let mut callables = Vec::new();
        for file in &self.project.files {
            let Some(artifact) = self.artifacts_by_path.get(&file.path) else {
                continue;
            };

            let module_owner_id = format!("{}:<module>", artifact.module_path);
            let local_module_owner_id = format!("{}:<module>", local_module_path(&artifact.path));
            if let Some((owner_id, callable)) = self
                .callables_by_owner
                .get(&module_owner_id)
                .map(|callable| (module_owner_id.clone(), callable))
                .or_else(|| {
                    self.callables_by_owner
                        .get(&local_module_owner_id)
                        .map(|callable| (local_module_owner_id.clone(), callable))
                })
            {
                callables.push(SemanticCallable::Module {
                    artifact,
                    callable,
                    owner_id,
                    file,
                });
            }

            for symbol in &file.symbols {
                if symbol.kind == SymbolKind::Class {
                    continue;
                }
                if let Some(callable) = self.callables_by_owner.get(&symbol.id) {
                    callables.push(SemanticCallable::Symbol {
                        artifact,
                        callable,
                        symbol,
                    });
                }
            }
        }
        callables
    }

    pub(crate) fn call_site_for(&self, callable_id: &str, call: &CallAst) -> Option<&sg::CallSite> {
        self.call_sites_by_key.get(&format!(
            "{}|{}|{}",
            callable_id,
            span_key(call.source_span),
            call.callee
        ))
    }
}

pub(crate) enum SemanticCallable<'a> {
    Module {
        artifact: &'a sg::Artifact,
        callable: &'a sg::Callable,
        owner_id: String,
        file: &'a crate::ast::FileAst,
    },
    Symbol {
        artifact: &'a sg::Artifact,
        callable: &'a sg::Callable,
        symbol: &'a SymbolAst,
    },
}

impl<'a> SemanticCallable<'a> {
    pub(crate) fn artifact(&self) -> &'a sg::Artifact {
        match self {
            Self::Module { artifact, .. } | Self::Symbol { artifact, .. } => artifact,
        }
    }

    pub(crate) fn callable(&self) -> &'a sg::Callable {
        match self {
            Self::Module { callable, .. } | Self::Symbol { callable, .. } => callable,
        }
    }

    pub(crate) fn owner_id(&self) -> &str {
        match self {
            Self::Module { owner_id, .. } => owner_id,
            Self::Symbol { symbol, .. } => &symbol.id,
        }
    }

    pub(crate) fn statements(&self) -> &'a [crate::ast::StatementAst] {
        match self {
            Self::Module { file, .. } => &file.statements,
            Self::Symbol { symbol, .. } => &symbol.statements,
        }
    }

    pub(crate) fn expressions(&self) -> &'a [crate::ast::ExpressionAst] {
        match self {
            Self::Module { file, .. } => &file.expressions,
            Self::Symbol { symbol, .. } => &symbol.expressions,
        }
    }

    pub(crate) fn conditions(&self) -> &'a [crate::ast::ConditionAst] {
        match self {
            Self::Module { file, .. } => &file.conditions,
            Self::Symbol { symbol, .. } => &symbol.conditions,
        }
    }

    pub(crate) fn definitions(&self) -> &'a [crate::ast::DefinitionAst] {
        match self {
            Self::Module { file, .. } => &file.definitions,
            Self::Symbol { symbol, .. } => &symbol.definitions,
        }
    }

    pub(crate) fn uses(&self) -> &'a [crate::ast::UseAst] {
        match self {
            Self::Module { file, .. } => &file.uses,
            Self::Symbol { symbol, .. } => &symbol.uses,
        }
    }

    pub(crate) fn returns(&self) -> &'a [crate::ast::ReturnAst] {
        match self {
            Self::Module { file, .. } => &file.returns,
            Self::Symbol { symbol, .. } => &symbol.returns,
        }
    }

    pub(crate) fn raises(&self) -> &'a [crate::ast::RaiseAst] {
        match self {
            Self::Module { file, .. } => &file.raises,
            Self::Symbol { symbol, .. } => &symbol.raises,
        }
    }

    pub(crate) fn field_accesses(&self) -> &'a [crate::ast::FieldAccessAst] {
        match self {
            Self::Module { file, .. } => &file.field_accesses,
            Self::Symbol { symbol, .. } => &symbol.field_accesses,
        }
    }

    pub(crate) fn index_accesses(&self) -> &'a [crate::ast::IndexAccessAst] {
        match self {
            Self::Module { file, .. } => &file.index_accesses,
            Self::Symbol { symbol, .. } => &symbol.index_accesses,
        }
    }

    pub(crate) fn calls(&self) -> &'a [CallAst] {
        match self {
            Self::Module { file, .. } => &file.calls,
            Self::Symbol { symbol, .. } => &symbol.calls,
        }
    }
}

pub fn insert_node(graph: &mut ProgramSupergraph, node: GraphNode) {
    graph.push_node_if_new(node);
}

pub fn insert_edge(graph: &mut ProgramSupergraph, edge: GraphEdge) {
    graph.push_edge_if_new(edge);
}

pub(crate) fn owner(artifact_id: &str, callable_id: &str) -> SourceOwnership {
    SourceOwnership {
        artifact_id: Some(artifact_id.to_string()),
        scope_id: None,
        callable_id: Some(callable_id.to_string()),
    }
}

pub fn inference_evidence(summary: impl Into<String>) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Inference,
        summary: summary.into(),
        source_id: None,
        source_span: None,
        content_hash: None,
        syntax: None,
    }]
}

pub(crate) fn span_key(span: SourceSpan) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        span.start_byte,
        span.end_byte,
        span.start_row,
        span.start_column,
        span.end_row,
        span.end_column
    )
}

pub(crate) fn span_contains(outer: SourceSpan, inner: SourceSpan) -> bool {
    outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}

pub fn edge_id(kind: &str, source_id: &str, target_id: &str, precision_key: &str) -> NodeId {
    stable_id("edge", &[kind, source_id, target_id, precision_key])
}

pub(crate) fn node_owner(semantic: &SemanticCallable<'_>) -> SourceOwnership {
    owner(
        &semantic.artifact().artifact_id,
        &semantic.callable().callable_id,
    )
}

pub(crate) fn graph_node(
    node_id: NodeId,
    kind: NodeKind,
    owner: SourceOwnership,
    span: Option<SourceSpan>,
    confidence: Confidence,
    evidence: Vec<Evidence>,
    fact: NodeFact,
) -> GraphNode {
    GraphNode {
        node_id,
        fact_id: String::new(),
        payload_hash: String::new(),
        kind,
        owner,
        span,
        confidence,
        uncertainty: uncertainty_from_confidence(&confidence),
        evidence,
        fact,
    }
}

pub(crate) fn graph_edge(
    edge_id: NodeId,
    kind: EdgeKind,
    source_id: NodeId,
    target_id: NodeId,
    owner: SourceOwnership,
    span: Option<SourceSpan>,
    confidence: Confidence,
    evidence: Vec<Evidence>,
    fact: EdgeFact,
) -> GraphEdge {
    GraphEdge {
        edge_id,
        fact_id: String::new(),
        payload_hash: String::new(),
        kind,
        source_id,
        target_id: Some(target_id),
        owner,
        span,
        confidence,
        uncertainty: uncertainty_from_confidence(&confidence),
        evidence,
        fact,
    }
}

fn call_site_key(call_site: &sg::CallSite) -> String {
    format!(
        "{}|{}|{}",
        call_site.enclosing_callable_id,
        span_key(call_site.span),
        call_site.callee_expression
    )
}

fn local_module_path(path: &str) -> String {
    path.trim_end_matches(".tsx")
        .trim_end_matches(".ts")
        .trim_end_matches(".py")
        .trim_end_matches("/__init__")
        .replace('\\', "/")
        .replace('/', ".")
}
