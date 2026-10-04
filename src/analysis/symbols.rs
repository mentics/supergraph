use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ast::{DefinitionAst, DefinitionKind as AstDefinitionKind, SourceSpan, UseAst};
use crate::supergraph::{
    self as sg, BindingKind, BindingTarget, Confidence, EdgeFact, EdgeKind, Evidence, EvidenceKind,
    ExternalTargetKind, NodeFact, NodeId, NodeKind, ProgramSupergraph, Resolution,
    ScopeBindingBehavior, ScopeKind, SourceOwnership, SymbolKind, SyntaxReference,
};

use super::{
    SemanticCallable, SemanticContext, edge_id, graph_edge, graph_node, insert_edge, insert_node,
    span_contains, span_key,
};

const PRECISION: &str = "sg030-lexical-symbol-table";
const RESOLUTION_PRECISION: &str = "sg031-resolution";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let mut tables = LexicalTables::new(graph);
    emit_external_targets_for_existing_bindings(graph);
    emit_symbols_for_existing_bindings(graph, &mut tables);

    for semantic in context.semantic_callables() {
        emit_declarations(graph, &mut tables, &semantic);
        emit_uses(graph, &mut tables, context, &semantic);
        emit_static_receiver_fields(graph, &mut tables, &semantic);
    }

    emit_resolution_edges(graph, &tables);
}

#[derive(Debug, Clone)]
struct ScopeInfo {
    scope_id: NodeId,
    parent_scope_id: Option<NodeId>,
    artifact_id: NodeId,
    owner_callable_id: Option<NodeId>,
    kind: ScopeKind,
    span: Option<SourceSpan>,
    binding_behavior: ScopeBindingBehavior,
}

#[derive(Debug, Clone)]
struct BindingInfo {
    binding_id: NodeId,
    scope_id: NodeId,
    name: String,
    kind: BindingKind,
    target: BindingTarget,
    span: SourceSpan,
}

/// Statements that carry an owning scope, in graph order, with a prefix maximum of their end
/// offsets so a containment query can skip the leading statements that end too early.
#[derive(Default)]
struct ScopedStatements {
    entries: Vec<(SourceSpan, NodeId)>,
    prefix_max_end: Vec<usize>,
}

impl ScopedStatements {
    fn push(&mut self, span: SourceSpan, scope_id: NodeId) {
        let previous = self.prefix_max_end.last().copied().unwrap_or(0);
        self.prefix_max_end.push(previous.max(span.end_byte));
        self.entries.push((span, scope_id));
    }

    /// Scope of the first statement (in graph order) whose span contains `span`.
    fn first_scope_containing(&self, span: SourceSpan) -> Option<NodeId> {
        let first = self
            .prefix_max_end
            .partition_point(|max_end| *max_end < span.end_byte);
        self.entries[first..]
            .iter()
            .find(|(candidate, _)| span_contains(*candidate, span))
            .map(|(_, scope_id)| scope_id.clone())
    }
}

struct LexicalTables {
    scopes: BTreeMap<NodeId, ScopeInfo>,
    scope_ids_by_artifact: HashMap<NodeId, Vec<NodeId>>,
    symbols_by_id: HashMap<NodeId, sg::Symbol>,
    statements_by_callable: HashMap<NodeId, Vec<(SourceSpan, NodeId)>>,
    scoped_statements: ScopedStatements,
    callable_by_name: HashMap<String, (usize, NodeId)>,
    callable_by_artifact_and_name: HashMap<(NodeId, String), NodeId>,
    class_scope_by_parent: HashMap<NodeId, (usize, NodeId)>,
    expression_positions: HashMap<(NodeId, SourceSpan), Vec<usize>>,
    bindings_by_scope_name: BTreeMap<(NodeId, String), Vec<BindingInfo>>,
    symbols_by_binding: BTreeMap<NodeId, NodeId>,
    placeholder_symbols: BTreeSet<(NodeId, String, SourceSpan)>,
}

impl LexicalTables {
    fn new(graph: &ProgramSupergraph) -> Self {
        let scopes = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::Scope(scope) => Some((
                    scope.scope_id.clone(),
                    ScopeInfo {
                        scope_id: scope.scope_id.clone(),
                        parent_scope_id: scope.parent_scope_id.clone(),
                        artifact_id: scope.artifact_id.clone(),
                        owner_callable_id: scope.owner_callable_id.clone(),
                        kind: scope.kind,
                        span: scope.span,
                        binding_behavior: scope.binding_behavior,
                    },
                )),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();

        let mut scope_ids_by_artifact = HashMap::<NodeId, Vec<NodeId>>::new();
        for scope in scopes.values() {
            scope_ids_by_artifact
                .entry(scope.artifact_id.clone())
                .or_default()
                .push(scope.scope_id.clone());
        }

        let mut tables = Self {
            scopes,
            scope_ids_by_artifact,
            symbols_by_id: HashMap::new(),
            statements_by_callable: HashMap::new(),
            scoped_statements: ScopedStatements::default(),
            callable_by_name: HashMap::new(),
            callable_by_artifact_and_name: HashMap::new(),
            class_scope_by_parent: HashMap::new(),
            expression_positions: HashMap::new(),
            bindings_by_scope_name: BTreeMap::new(),
            symbols_by_binding: BTreeMap::new(),
            placeholder_symbols: BTreeSet::new(),
        };

        for (position, node) in graph.nodes.iter().enumerate() {
            match &node.fact {
                NodeFact::Scope(scope) => {
                    if scope.kind == ScopeKind::Class && node.span.is_some() {
                        if let Some(parent) = &scope.parent_scope_id {
                            tables
                                .class_scope_by_parent
                                .entry(parent.clone())
                                .or_insert_with(|| (position, scope.scope_id.clone()));
                        }
                    }
                }
                NodeFact::Callable(callable) => {
                    if let Some(name) = &callable.name {
                        tables
                            .callable_by_name
                            .entry(name.clone())
                            .or_insert_with(|| (position, callable.callable_id.clone()));
                        tables
                            .callable_by_artifact_and_name
                            .entry((callable.artifact_id.clone(), name.clone()))
                            .or_insert_with(|| callable.callable_id.clone());
                    }
                }
                NodeFact::Statement(statement) => {
                    if let Some(span) = node.span {
                        tables
                            .statements_by_callable
                            .entry(statement.callable_id.clone())
                            .or_default()
                            .push((span, statement.statement_id.clone()));
                        if let Some(scope_id) = &node.owner.scope_id {
                            tables.scoped_statements.push(span, scope_id.clone());
                        }
                    }
                }
                NodeFact::Expression(expression) => {
                    if let Some(span) = node.span {
                        tables
                            .expression_positions
                            .entry((expression.callable_id.clone(), span))
                            .or_default()
                            .push(position);
                    }
                }
                NodeFact::Binding(binding) => tables.register_binding(BindingInfo {
                    binding_id: binding.binding_id.clone(),
                    scope_id: binding.scope_id.clone(),
                    name: binding.name.clone(),
                    kind: binding.kind,
                    target: binding.target.clone(),
                    span: binding.span,
                }),
                NodeFact::Symbol(symbol) => {
                    tables
                        .symbols_by_id
                        .entry(symbol.symbol_id.clone())
                        .or_insert_with(|| symbol.clone());
                    if let Some(binding_id) = &symbol.binding_id {
                        tables
                            .symbols_by_binding
                            .insert(binding_id.clone(), symbol.symbol_id.clone());
                    }
                }
                _ => {}
            }
        }

        tables
    }

    fn register_binding(&mut self, binding: BindingInfo) {
        self.bindings_by_scope_name
            .entry((binding.scope_id.clone(), binding.name.clone()))
            .or_default()
            .push(binding);
    }

    fn scope_owner(&self, scope_id: &str) -> SourceOwnership {
        self.scopes
            .get(scope_id)
            .map(|scope| SourceOwnership {
                artifact_id: Some(scope.artifact_id.clone()),
                scope_id: Some(scope.scope_id.clone()),
                callable_id: scope.owner_callable_id.clone(),
            })
            .unwrap_or_else(|| SourceOwnership {
                artifact_id: None,
                scope_id: Some(scope_id.to_string()),
                callable_id: None,
            })
    }

    fn binding_scope_for_span(&self, artifact_id: &str, span: SourceSpan) -> Option<NodeId> {
        let artifact_scopes = self
            .scope_ids_by_artifact
            .get(artifact_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|scope_id| self.scopes.get(scope_id));
        let nearest = artifact_scopes
            .clone()
            .filter(|scope| {
                scope
                    .span
                    .is_some_and(|candidate| span_contains(candidate, span))
            })
            .min_by_key(|scope| {
                scope
                    .span
                    .map(|span| span.end_byte.saturating_sub(span.start_byte))
                    .unwrap_or(usize::MAX)
            })
            .map(|scope| scope.scope_id.clone())
            .or_else(|| {
                artifact_scopes
                    .clone()
                    .find(|scope| scope.kind == ScopeKind::Module)
                    .map(|scope| scope.scope_id.clone())
            })?;
        Some(self.normalize_binding_scope(&nearest))
    }

    fn normalize_binding_scope(&self, scope_id: &str) -> NodeId {
        let mut current = scope_id.to_string();
        while let Some(scope) = self.scopes.get(&current) {
            if scope.binding_behavior != ScopeBindingBehavior::Transparent {
                return current;
            }
            let Some(parent) = &scope.parent_scope_id else {
                return current;
            };
            current = parent.clone();
        }
        current
    }

    fn visible_bindings(&self, scope_id: &str, name: &str) -> Vec<BindingInfo> {
        let mut current = Some(self.normalize_binding_scope(scope_id));
        while let Some(scope_id) = current {
            if let Some(bindings) = self
                .bindings_by_scope_name
                .get(&(scope_id.clone(), name.to_string()))
            {
                let mut bindings = bindings.clone();
                bindings.sort_by(|left, right| {
                    right
                        .span
                        .start_byte
                        .cmp(&left.span.start_byte)
                        .then_with(|| left.binding_id.cmp(&right.binding_id))
                });
                bindings.dedup_by(|left, right| left.binding_id == right.binding_id);
                return bindings;
            }
            current = self
                .scopes
                .get(&scope_id)
                .and_then(|scope| scope.parent_scope_id.clone());
        }
        Vec::new()
    }

    fn visible_binding(&self, scope_id: &str, name: &str) -> Option<BindingInfo> {
        self.visible_bindings(scope_id, name).into_iter().next()
    }

    fn binding_with_same_shape(
        &self,
        scope_id: &str,
        name: &str,
        kind: BindingKind,
        span: SourceSpan,
    ) -> Option<BindingInfo> {
        self.bindings_by_scope_name
            .get(&(scope_id.to_string(), name.to_string()))
            .and_then(|bindings| {
                bindings
                    .iter()
                    .find(|binding| {
                        binding.kind == kind
                            && (binding.span == span
                                || span_contains(binding.span, span)
                                || span_contains(span, binding.span))
                    })
                    .cloned()
            })
    }
}

fn emit_external_targets_for_existing_bindings(graph: &mut ProgramSupergraph) {
    let existing = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::ExternalTarget(target) => Some(target.external_target_id.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let targets = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Binding(binding) => match &binding.target {
                BindingTarget::External(target_id) if !existing.contains(target_id) => {
                    Some(target_id.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    for target_id in targets {
        insert_node(
            graph,
            graph_node(
                target_id.clone(),
                NodeKind::ExternalTarget,
                SourceOwnership::default(),
                None,
                Confidence::Unknown,
                resolver_evidence(
                    "external binding target placeholder",
                    None,
                    "external-target",
                ),
                NodeFact::ExternalTarget(sg::ExternalTarget {
                    external_target_id: target_id.clone(),
                    ecosystem: "unknown".to_string(),
                    package_name: None,
                    package_version: None,
                    module_path: None,
                    qualified_name: target_id,
                    member_path: None,
                    target_kind: ExternalTargetKind::Unknown,
                    source: PRECISION.to_string(),
                }),
            ),
        );
    }
}

fn emit_symbols_for_existing_bindings(graph: &mut ProgramSupergraph, tables: &mut LexicalTables) {
    let bindings = tables
        .bindings_by_scope_name
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    for binding in bindings {
        ensure_symbol_for_binding(graph, tables, &binding);
        add_binding_resolves_to(graph, tables, &binding);
    }
}

fn emit_declarations(
    graph: &mut ProgramSupergraph,
    tables: &mut LexicalTables,
    semantic: &SemanticCallable<'_>,
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let artifact_id = semantic.artifact().artifact_id.as_str();

    for definition in semantic
        .definitions()
        .iter()
        .filter(|definition| definition.owner_id == semantic.owner_id())
    {
        let scope_id = binding_scope_for_definition(tables, semantic, definition)
            .or_else(|| tables.binding_scope_for_span(artifact_id, definition.source_span))
            .unwrap_or_else(|| semantic.callable().scope_id.clone());
        let binding_kind = binding_kind(definition.kind);
        let binding = tables
            .binding_with_same_shape(
                &scope_id,
                &definition.name,
                binding_kind,
                definition.source_span,
            )
            .unwrap_or_else(|| {
                let binding = BindingInfo {
                    binding_id: sg::binding_id(&scope_id, &definition.name, definition.source_span),
                    scope_id: scope_id.clone(),
                    name: definition.name.clone(),
                    kind: binding_kind,
                    target: binding_target_for_definition(tables, definition, semantic, &scope_id),
                    span: definition.source_span,
                };
                insert_binding(graph, tables, binding.clone());
                binding
            });

        let symbol_id = ensure_symbol_for_binding(graph, tables, &binding);
        add_binding_resolves_to(graph, tables, &binding);
        emit_definition_node(graph, tables, semantic, definition, &symbol_id, callable_id);
        update_expression_symbol(graph, tables, callable_id, definition.source_span, &symbol_id);
    }
}

fn emit_uses(
    graph: &mut ProgramSupergraph,
    tables: &mut LexicalTables,
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let artifact_id = semantic.artifact().artifact_id.as_str();

    for use_fact in semantic
        .uses()
        .iter()
        .filter(|use_fact| use_fact.owner_id == semantic.owner_id())
    {
        let scope_id = scope_for_use(tables, artifact_id, use_fact.source_span)
            .unwrap_or_else(|| semantic.callable().scope_id.clone());
        let visible_bindings = tables.visible_bindings(&scope_id, &use_fact.name);
        let symbol_id = match visible_bindings.as_slice() {
            [] => ensure_placeholder_symbol(
                graph,
                tables,
                &scope_id,
                &use_fact.name,
                use_fact.source_span,
                symbol_kind_for_use(use_fact.kind),
                Resolution::Unresolved,
            ),
            [binding] => ensure_symbol_for_binding(graph, tables, binding),
            _ => ensure_placeholder_symbol(
                graph,
                tables,
                &scope_id,
                &use_fact.name,
                use_fact.source_span,
                symbol_kind_for_use(use_fact.kind),
                Resolution::Ambiguous,
            ),
        };

        let use_id = sg::use_id(callable_id, &use_fact.name, use_fact.source_span);
        insert_node(
            graph,
            graph_node(
                use_id.clone(),
                NodeKind::Use,
                source_owner(
                    tables,
                    &scope_id,
                    Some(semantic.callable().callable_id.clone()),
                ),
                Some(use_fact.source_span),
                Confidence::Exact,
                parser_evidence(
                    semantic,
                    Some(use_fact.source_span),
                    "use",
                    "lexical use fact",
                ),
                NodeFact::Use(sg::Use {
                    use_id: use_id.clone(),
                    callable_id: callable_id.to_string(),
                    symbol_id: Some(symbol_id.clone()),
                    value_id: None,
                    kind: use_kind(use_fact.kind),
                    name: Some(use_fact.name.clone()),
                }),
            ),
        );
        add_uses_edge(graph, tables, context, semantic, use_fact, &use_id);
        update_expression_symbol(graph, tables, callable_id, use_fact.source_span, &symbol_id);
    }
}

fn emit_static_receiver_fields(
    graph: &mut ProgramSupergraph,
    tables: &mut LexicalTables,
    semantic: &SemanticCallable<'_>,
) {
    let Some(class_scope_id) = class_scope_for_method(tables, semantic) else {
        return;
    };
    let callable_id = semantic.callable().callable_id.as_str();

    for field in semantic
        .field_accesses()
        .iter()
        .filter(|field| field.owner_id == semantic.owner_id())
        .filter(|field| is_static_receiver(field.object.as_deref()))
    {
        let binding = tables
            .visible_binding(&class_scope_id, &field.field)
            .filter(|binding| binding.kind == BindingKind::Field)
            .unwrap_or_else(|| {
                let binding = BindingInfo {
                    binding_id: sg::binding_id(&class_scope_id, &field.field, field.source_span),
                    scope_id: class_scope_id.clone(),
                    name: field.field.clone(),
                    kind: BindingKind::Field,
                    target: BindingTarget::Value(field.text.clone()),
                    span: field.source_span,
                };
                insert_binding(graph, tables, binding.clone());
                binding
            });
        let symbol_id = ensure_symbol_for_binding(graph, tables, &binding);
        let use_id = sg::use_id(callable_id, &field.field, field.source_span);
        insert_node(
            graph,
            graph_node(
                use_id.clone(),
                NodeKind::Use,
                source_owner(
                    tables,
                    &class_scope_id,
                    Some(semantic.callable().callable_id.clone()),
                ),
                Some(field.source_span),
                Confidence::Exact,
                parser_evidence(
                    semantic,
                    Some(field.source_span),
                    "field",
                    "statically visible receiver field use",
                ),
                NodeFact::Use(sg::Use {
                    use_id: use_id.clone(),
                    callable_id: callable_id.to_string(),
                    symbol_id: Some(symbol_id),
                    value_id: None,
                    kind: sg::UseKind::FieldRead,
                    name: Some(field.field.clone()),
                }),
            ),
        );
        insert_edge(
            graph,
            graph_edge(
                edge_id(
                    "uses",
                    &semantic.callable().callable_id,
                    &use_id,
                    &field.field,
                ),
                EdgeKind::Uses,
                semantic.callable().callable_id.clone(),
                use_id,
                source_owner(
                    tables,
                    &class_scope_id,
                    Some(semantic.callable().callable_id.clone()),
                ),
                Some(field.source_span),
                Confidence::Exact,
                parser_evidence(
                    semantic,
                    Some(field.source_span),
                    "field",
                    "statically visible receiver field use edge",
                ),
                EdgeFact::Uses(sg::Uses {
                    callable_id: semantic.callable().callable_id.clone(),
                    use_id: sg::use_id(callable_id, &field.field, field.source_span),
                    name: field.field.clone(),
                }),
            ),
        );
    }
}

fn insert_binding(graph: &mut ProgramSupergraph, tables: &mut LexicalTables, binding: BindingInfo) {
    let owner = tables.scope_owner(&binding.scope_id);
    insert_node(
        graph,
        graph_node(
            binding.binding_id.clone(),
            NodeKind::Binding,
            owner.clone(),
            Some(binding.span),
            binding_confidence(&binding.target),
            resolver_evidence("lexical binding fact", Some(binding.span), "binding"),
            NodeFact::Binding(sg::Binding {
                binding_id: binding.binding_id.clone(),
                scope_id: binding.scope_id.clone(),
                name: binding.name.clone(),
                kind: binding.kind,
                target: binding.target.clone(),
                span: binding.span,
            }),
        ),
    );
    insert_edge(
        graph,
        graph_edge(
            edge_id("binds", &binding.scope_id, &binding.binding_id, PRECISION),
            EdgeKind::Binds,
            binding.scope_id.clone(),
            binding.binding_id.clone(),
            owner,
            Some(binding.span),
            Confidence::Exact,
            resolver_evidence("scope owns lexical binding", Some(binding.span), "binding"),
            EdgeFact::Binds(sg::Binds {
                scope_id: binding.scope_id.clone(),
                binding_id: binding.binding_id.clone(),
            }),
        ),
    );
    tables.register_binding(binding);
}

fn ensure_symbol_for_binding(
    graph: &mut ProgramSupergraph,
    tables: &mut LexicalTables,
    binding: &BindingInfo,
) -> NodeId {
    if let Some(symbol_id) = tables.symbols_by_binding.get(&binding.binding_id) {
        return symbol_id.clone();
    }
    let symbol_id = sg::symbol_id(&binding.scope_id, &binding.name, Some(binding.span));
    tables
        .symbols_by_id
        .entry(symbol_id.clone())
        .or_insert_with(|| sg::Symbol {
            symbol_id: symbol_id.clone(),
            scope_id: binding.scope_id.clone(),
            name: binding.name.clone(),
            kind: symbol_kind_for_binding(binding.kind),
            binding_id: Some(binding.binding_id.clone()),
            resolution: resolution_from_target(&binding.target),
        });
    insert_node(
        graph,
        graph_node(
            symbol_id.clone(),
            NodeKind::Symbol,
            tables.scope_owner(&binding.scope_id),
            Some(binding.span),
            binding_confidence(&binding.target),
            resolver_evidence(
                "symbol table entry for binding",
                Some(binding.span),
                "symbol",
            ),
            NodeFact::Symbol(sg::Symbol {
                symbol_id: symbol_id.clone(),
                scope_id: binding.scope_id.clone(),
                name: binding.name.clone(),
                kind: symbol_kind_for_binding(binding.kind),
                binding_id: Some(binding.binding_id.clone()),
                resolution: resolution_from_target(&binding.target),
            }),
        ),
    );
    tables
        .symbols_by_binding
        .insert(binding.binding_id.clone(), symbol_id.clone());
    symbol_id
}

fn ensure_placeholder_symbol(
    graph: &mut ProgramSupergraph,
    tables: &mut LexicalTables,
    scope_id: &str,
    name: &str,
    span: SourceSpan,
    kind: SymbolKind,
    resolution: Resolution,
) -> NodeId {
    let key = (scope_id.to_string(), name.to_string(), span);
    let symbol_id = sg::symbol_id(scope_id, name, Some(span));
    if tables.placeholder_symbols.insert(key) {
        tables
            .symbols_by_id
            .entry(symbol_id.clone())
            .or_insert_with(|| sg::Symbol {
                symbol_id: symbol_id.clone(),
                scope_id: scope_id.to_string(),
                name: name.to_string(),
                kind,
                binding_id: None,
                resolution,
            });
        insert_node(
            graph,
            graph_node(
                symbol_id.clone(),
                NodeKind::Symbol,
                tables.scope_owner(scope_id),
                Some(span),
                Confidence::Unknown,
                resolver_evidence(
                    "unresolved lexical symbol placeholder",
                    Some(span),
                    "symbol",
                ),
                NodeFact::Symbol(sg::Symbol {
                    symbol_id: symbol_id.clone(),
                    scope_id: scope_id.to_string(),
                    name: name.to_string(),
                    kind,
                    binding_id: None,
                    resolution,
                }),
            ),
        );
    }
    symbol_id
}

fn emit_definition_node(
    graph: &mut ProgramSupergraph,
    tables: &LexicalTables,
    semantic: &SemanticCallable<'_>,
    definition: &DefinitionAst,
    symbol_id: &str,
    callable_id: &str,
) {
    let definition_id = sg::definition_id(callable_id, &definition.name, definition.source_span);
    let scope_id = tables
        .symbols_by_id
        .get(symbol_id)
        .map(|symbol| symbol.scope_id.clone())
        .unwrap_or_else(|| semantic.callable().scope_id.clone());
    insert_node(
        graph,
        graph_node(
            definition_id.clone(),
            NodeKind::Definition,
            source_owner(
                tables,
                &scope_id,
                Some(semantic.callable().callable_id.clone()),
            ),
            Some(definition.source_span),
            Confidence::Exact,
            parser_evidence(
                semantic,
                Some(definition.source_span),
                "definition",
                "lexical definition fact",
            ),
            NodeFact::Definition(sg::Definition {
                definition_id: definition_id.clone(),
                callable_id: callable_id.to_string(),
                symbol_id: Some(symbol_id.to_string()),
                value_id: None,
                kind: definition_kind(definition.kind),
                name: Some(definition.name.clone()),
            }),
        ),
    );
    let source_id = containing_statement_id(tables, semantic, definition.source_span)
        .unwrap_or_else(|| semantic.callable().callable_id.clone());
    insert_edge(
        graph,
        graph_edge(
            edge_id("defines", &source_id, &definition_id, &definition.name),
            EdgeKind::Defines,
            source_id,
            definition_id.clone(),
            source_owner(
                tables,
                &scope_id,
                Some(semantic.callable().callable_id.clone()),
            ),
            Some(definition.source_span),
            Confidence::Exact,
            parser_evidence(
                semantic,
                Some(definition.source_span),
                "definition",
                "definition binds lexical symbol",
            ),
            EdgeFact::Defines(sg::Defines {
                callable_id: callable_id.to_string(),
                definition_id,
                name: definition.name.clone(),
            }),
        ),
    );
}

fn add_uses_edge(
    graph: &mut ProgramSupergraph,
    tables: &LexicalTables,
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
    use_fact: &UseAst,
    use_id: &str,
) {
    let scope_id = tables
        .scoped_statements
        .first_scope_containing(use_fact.source_span)
        .or_else(|| Some(semantic.callable().scope_id.clone()));
    let owner = SourceOwnership {
        artifact_id: Some(semantic.artifact().artifact_id.clone()),
        scope_id,
        callable_id: Some(semantic.callable().callable_id.clone()),
    };
    let source_id = containing_statement_id(tables, semantic, use_fact.source_span)
        .or_else(|| containing_call_site_id(context, semantic, use_fact.source_span))
        .unwrap_or_else(|| semantic.callable().callable_id.clone());
    insert_edge(
        graph,
        graph_edge(
            edge_id("uses", &source_id, use_id, &use_fact.name),
            EdgeKind::Uses,
            source_id,
            use_id.to_string(),
            owner,
            Some(use_fact.source_span),
            Confidence::Exact,
            parser_evidence(
                semantic,
                Some(use_fact.source_span),
                "use",
                "use references lexical symbol",
            ),
            EdgeFact::Uses(sg::Uses {
                callable_id: semantic.callable().callable_id.clone(),
                use_id: use_id.to_string(),
                name: use_fact.name.clone(),
            }),
        ),
    );
}

fn add_binding_resolves_to(
    graph: &mut ProgramSupergraph,
    tables: &LexicalTables,
    binding: &BindingInfo,
) {
    let Some(target_id) = target_node_id(&binding.target) else {
        return;
    };
    insert_edge(
        graph,
        graph_edge(
            edge_id("resolves-to", &binding.binding_id, &target_id, PRECISION),
            EdgeKind::ResolvesTo,
            binding.binding_id.clone(),
            target_id.clone(),
            tables.scope_owner(&binding.scope_id),
            Some(binding.span),
            binding_confidence(&binding.target),
            resolver_evidence("binding has lexical target", Some(binding.span), "binding"),
            EdgeFact::ResolvesTo(sg::ResolvesTo {
                binding_id: binding.binding_id.clone(),
                target_id,
                resolution: resolution_from_target(&binding.target),
            }),
        ),
    );
}

fn emit_resolution_edges(graph: &mut ProgramSupergraph, tables: &LexicalTables) {
    let bindings = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Binding(binding) => Some((binding.binding_id.clone(), binding.clone())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let symbols = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Symbol(symbol) => Some((symbol.clone(), node.owner.clone(), node.span)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let uses = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Use(use_fact) => Some((use_fact.clone(), node.owner.clone(), node.span)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let call_sites = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::CallSite(call_site) => {
                Some((call_site.call_site_id.clone(), call_site.clone()))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let calls = graph
        .edges
        .iter()
        .filter_map(|edge| match &edge.fact {
            EdgeFact::Calls(calls) => Some((calls.clone(), edge.owner.clone(), edge.span)),
            _ => None,
        })
        .collect::<Vec<_>>();

    for (symbol, owner, span) in symbols {
        if let Some(binding_id) = &symbol.binding_id {
            if let Some(binding) = bindings.get(binding_id) {
                add_resolution_edge(
                    graph,
                    &symbol.symbol_id,
                    &binding.binding_id,
                    symbol.resolution,
                    owner.clone(),
                    span,
                    "symbol resolves to lexical binding",
                );
                add_binding_target_resolution_edges(
                    graph,
                    &symbol.symbol_id,
                    binding,
                    owner,
                    span,
                    "symbol resolves through binding target",
                );
            }
        } else {
            let candidate_bindings = tables.visible_bindings(&symbol.scope_id, &symbol.name);
            if candidate_bindings.is_empty() {
                let target_id = ensure_resolution_placeholder_target(
                    graph,
                    &symbol.name,
                    symbol.resolution,
                    owner.clone(),
                    span,
                );
                add_resolution_edge(
                    graph,
                    &symbol.symbol_id,
                    &target_id,
                    symbol.resolution,
                    owner,
                    span,
                    "symbol has unresolved resolution placeholder",
                );
            } else {
                for binding in candidate_bindings {
                    add_resolution_edge(
                        graph,
                        &symbol.symbol_id,
                        &binding.binding_id,
                        Resolution::Ambiguous,
                        owner.clone(),
                        span,
                        "symbol has ambiguous lexical candidates",
                    );
                }
            }
        }
    }

    for (use_fact, owner, span) in uses {
        let Some(symbol_id) = &use_fact.symbol_id else {
            let target_id = ensure_resolution_placeholder_target(
                graph,
                use_fact.name.as_deref().unwrap_or("<unknown>"),
                Resolution::Unresolved,
                owner.clone(),
                span,
            );
            add_resolution_edge(
                graph,
                &use_fact.use_id,
                &target_id,
                Resolution::Unresolved,
                owner,
                span,
                "use has no lexical symbol",
            );
            continue;
        };
        let symbol = tables.symbols_by_id.get(symbol_id).cloned();
        let Some(symbol) = symbol else {
            continue;
        };
        if let Some(binding_id) = &symbol.binding_id {
            if let Some(binding) = bindings.get(binding_id) {
                add_resolution_edge(
                    graph,
                    &use_fact.use_id,
                    &binding.binding_id,
                    symbol.resolution,
                    owner.clone(),
                    span,
                    "use resolves to lexical binding",
                );
                add_binding_target_resolution_edges(
                    graph,
                    &use_fact.use_id,
                    binding,
                    owner,
                    span,
                    "use resolves through binding target",
                );
            }
        } else {
            let candidate_bindings = use_fact
                .name
                .as_deref()
                .map(|name| tables.visible_bindings(&symbol.scope_id, name))
                .unwrap_or_default();
            if candidate_bindings.is_empty() {
                let target_id = ensure_resolution_placeholder_target(
                    graph,
                    use_fact.name.as_deref().unwrap_or("<unknown>"),
                    symbol.resolution,
                    owner.clone(),
                    span,
                );
                add_resolution_edge(
                    graph,
                    &use_fact.use_id,
                    &target_id,
                    symbol.resolution,
                    owner,
                    span,
                    "use has unresolved resolution placeholder",
                );
            } else {
                for binding in candidate_bindings {
                    add_resolution_edge(
                        graph,
                        &use_fact.use_id,
                        &binding.binding_id,
                        Resolution::Ambiguous,
                        owner.clone(),
                        span,
                        "use has ambiguous lexical candidates",
                    );
                }
            }
        }
    }

    for (calls, owner, span) in calls {
        let Some(call_site) = call_sites.get(&calls.call_site_id) else {
            continue;
        };
        let target_id = calls
            .callee_callable_id
            .clone()
            .or_else(|| calls.external_target_id.clone())
            .unwrap_or_else(|| {
                ensure_resolution_placeholder_target(
                    graph,
                    calls
                        .unresolved_target
                        .as_deref()
                        .unwrap_or(&call_site.callee_expression),
                    calls.resolution,
                    owner.clone(),
                    span,
                )
            });
        add_resolution_edge(
            graph,
            &calls.call_site_id,
            &target_id,
            calls.resolution,
            owner.clone(),
            span,
            "call site resolves through call target fact",
        );

        if let Some(binding) = possible_dynamic_callee_binding(tables, call_site) {
            add_resolution_edge(
                graph,
                &calls.call_site_id,
                &binding.binding_id,
                Resolution::Possible,
                owner,
                span,
                "call site has possible dynamic callee binding",
            );
        }
    }
}

fn add_binding_target_resolution_edges(
    graph: &mut ProgramSupergraph,
    source_id: &str,
    binding: &sg::Binding,
    owner: SourceOwnership,
    span: Option<SourceSpan>,
    summary: &str,
) {
    let Some(target_id) = target_node_id(&binding.target) else {
        return;
    };
    add_resolution_edge(
        graph,
        source_id,
        &target_id,
        resolution_from_target(&binding.target),
        owner,
        span,
        summary,
    );
}

fn add_resolution_edge(
    graph: &mut ProgramSupergraph,
    source_id: &str,
    target_id: &str,
    resolution: Resolution,
    owner: SourceOwnership,
    span: Option<SourceSpan>,
    summary: &str,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id(
                "resolves-to",
                source_id,
                target_id,
                &format!("{RESOLUTION_PRECISION}:{resolution:?}"),
            ),
            EdgeKind::ResolvesTo,
            source_id.to_string(),
            target_id.to_string(),
            owner,
            span,
            confidence_from_resolution(resolution),
            resolver_evidence(summary, span, "resolution"),
            EdgeFact::ResolvesTo(sg::ResolvesTo {
                binding_id: source_id.to_string(),
                target_id: target_id.to_string(),
                resolution,
            }),
        ),
    );
}

fn ensure_resolution_placeholder_target(
    graph: &mut ProgramSupergraph,
    name: &str,
    resolution: Resolution,
    owner: SourceOwnership,
    span: Option<SourceSpan>,
) -> NodeId {
    let target_id = sg::stable_id(
        "external-target",
        &[
            RESOLUTION_PRECISION,
            &format!("{resolution:?}"),
            name,
            &span.map(span_key).unwrap_or_default(),
        ],
    );
    insert_node(
        graph,
        graph_node(
            target_id.clone(),
            NodeKind::ExternalTarget,
            owner,
            span,
            confidence_from_resolution(resolution),
            resolver_evidence("resolution placeholder target", span, "external-target"),
            NodeFact::ExternalTarget(sg::ExternalTarget {
                external_target_id: target_id.clone(),
                ecosystem: "unknown".to_string(),
                package_name: None,
                package_version: None,
                module_path: None,
                qualified_name: name.to_string(),
                member_path: None,
                target_kind: ExternalTargetKind::Unknown,
                source: RESOLUTION_PRECISION.to_string(),
            }),
        ),
    );
    target_id
}

fn possible_dynamic_callee_binding(
    tables: &LexicalTables,
    call_site: &sg::CallSite,
) -> Option<BindingInfo> {
    if call_site.callee_expression.contains('.') {
        return None;
    }
    let scope_id = tables.binding_scope_for_span(&call_site.artifact_id, call_site.span)?;
    tables
        .visible_binding(&scope_id, &call_site.callee_expression)
        .filter(|binding| matches!(binding.target, BindingTarget::Value(_)))
}

fn update_expression_symbol(
    graph: &mut ProgramSupergraph,
    tables: &LexicalTables,
    callable_id: &str,
    span: SourceSpan,
    symbol_id: &str,
) {
    let Some(positions) = tables
        .expression_positions
        .get(&(callable_id.to_string(), span))
    else {
        return;
    };
    for position in positions {
        if let NodeFact::Expression(expression) = &mut graph.nodes[*position].fact {
            expression.symbol_id = Some(symbol_id.to_string());
        }
    }
}

fn binding_scope_for_definition(
    tables: &LexicalTables,
    semantic: &SemanticCallable<'_>,
    definition: &DefinitionAst,
) -> Option<NodeId> {
    if definition.kind == AstDefinitionKind::Parameter {
        return Some(semantic.callable().scope_id.clone());
    }
    if definition.kind == AstDefinitionKind::Field {
        if let Some(class_scope_id) = class_scope_for_method(tables, semantic) {
            return Some(class_scope_id);
        }
    }
    tables.binding_scope_for_span(&semantic.artifact().artifact_id, definition.source_span)
}

fn scope_for_use(tables: &LexicalTables, artifact_id: &str, span: SourceSpan) -> Option<NodeId> {
    tables.binding_scope_for_span(artifact_id, span)
}

fn class_scope_for_method(
    tables: &LexicalTables,
    semantic: &SemanticCallable<'_>,
) -> Option<NodeId> {
    tables
        .scopes
        .get(&semantic.callable().scope_id)
        .and_then(|scope| scope.parent_scope_id.clone())
        .filter(|scope_id| {
            tables
                .scopes
                .get(scope_id)
                .is_some_and(|scope| scope.kind == ScopeKind::Class)
        })
}

fn binding_target_for_definition(
    tables: &LexicalTables,
    definition: &DefinitionAst,
    semantic: &SemanticCallable<'_>,
    scope_id: &str,
) -> BindingTarget {
    match definition.kind {
        AstDefinitionKind::Class => callable_or_scope_target(tables, &definition.name, scope_id)
            .unwrap_or_else(|| BindingTarget::Unresolved(definition.name.clone())),
        AstDefinitionKind::Function => callable_target(tables, semantic, &definition.name)
            .unwrap_or_else(|| BindingTarget::Unresolved(definition.name.clone())),
        AstDefinitionKind::Import => BindingTarget::Unresolved(definition.text.clone()),
        AstDefinitionKind::Unknown => BindingTarget::Unresolved(definition.text.clone()),
        _ => BindingTarget::Value(definition.text.clone()),
    }
}

fn callable_or_scope_target(
    tables: &LexicalTables,
    name: &str,
    scope_id: &str,
) -> Option<BindingTarget> {
    // The first matching node in graph order wins, whichever kind it is.
    let class_scope = tables.class_scope_by_parent.get(scope_id);
    let callable = tables.callable_by_name.get(name);
    match (class_scope, callable) {
        (Some((scope_position, scope_id)), Some((callable_position, _)))
            if scope_position < callable_position =>
        {
            Some(BindingTarget::Class(scope_id.clone()))
        }
        (_, Some((_, callable_id))) => Some(BindingTarget::Callable(callable_id.clone())),
        (Some((_, scope_id)), None) => Some(BindingTarget::Class(scope_id.clone())),
        (None, None) => None,
    }
}

fn callable_target(
    tables: &LexicalTables,
    semantic: &SemanticCallable<'_>,
    name: &str,
) -> Option<BindingTarget> {
    tables
        .callable_by_artifact_and_name
        .get(&(semantic.artifact().artifact_id.clone(), name.to_string()))
        .map(|callable_id| BindingTarget::Callable(callable_id.clone()))
}

fn target_node_id(target: &BindingTarget) -> Option<NodeId> {
    match target {
        BindingTarget::Callable(target)
        | BindingTarget::Class(target)
        | BindingTarget::Module(target)
        | BindingTarget::External(target) => Some(target.clone()),
        BindingTarget::Value(_) | BindingTarget::Unresolved(_) => None,
    }
}

fn source_owner(
    tables: &LexicalTables,
    scope_id: &str,
    callable_id: Option<NodeId>,
) -> SourceOwnership {
    let mut owner = tables.scope_owner(scope_id);
    if callable_id.is_some() {
        owner.callable_id = callable_id;
    }
    owner
}

fn containing_statement_id(
    tables: &LexicalTables,
    semantic: &SemanticCallable<'_>,
    span: SourceSpan,
) -> Option<NodeId> {
    tables
        .statements_by_callable
        .get(&semantic.callable().callable_id)?
        .iter()
        .find(|(statement_span, _)| span_contains(*statement_span, span))
        .map(|(_, statement_id)| statement_id.clone())
}

fn containing_call_site_id(
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
    span: SourceSpan,
) -> Option<NodeId> {
    semantic
        .calls()
        .iter()
        .filter(|call| span_contains(call.source_span, span))
        .filter_map(|call| context.call_site_for(&semantic.callable().callable_id, call))
        .map(|call_site| call_site.call_site_id.clone())
        .next()
}

fn is_static_receiver(receiver: Option<&str>) -> bool {
    receiver
        .map(|receiver| {
            let base = receiver.split('.').next().unwrap_or(receiver);
            matches!(base, "self" | "this" | "cls")
        })
        .unwrap_or(false)
}

fn binding_kind(kind: AstDefinitionKind) -> BindingKind {
    match kind {
        AstDefinitionKind::Class => BindingKind::Class,
        AstDefinitionKind::Function => BindingKind::Function,
        AstDefinitionKind::Parameter => BindingKind::Parameter,
        AstDefinitionKind::Field => BindingKind::Field,
        AstDefinitionKind::Import => BindingKind::Import,
        AstDefinitionKind::Variable | AstDefinitionKind::Type => BindingKind::Assignment,
        AstDefinitionKind::Unknown => BindingKind::Unknown,
    }
}

fn definition_kind(kind: AstDefinitionKind) -> sg::DefinitionKind {
    match kind {
        AstDefinitionKind::Parameter => sg::DefinitionKind::Parameter,
        AstDefinitionKind::Import => sg::DefinitionKind::Import,
        AstDefinitionKind::Field => sg::DefinitionKind::FieldWrite,
        AstDefinitionKind::Variable | AstDefinitionKind::Type => sg::DefinitionKind::Assignment,
        AstDefinitionKind::Class | AstDefinitionKind::Function => sg::DefinitionKind::Declaration,
        AstDefinitionKind::Unknown => sg::DefinitionKind::Unknown,
    }
}

fn use_kind(kind: crate::ast::UseKind) -> sg::UseKind {
    match kind {
        crate::ast::UseKind::Callee => sg::UseKind::CallCallee,
        crate::ast::UseKind::Field => sg::UseKind::FieldRead,
        crate::ast::UseKind::Identifier | crate::ast::UseKind::Type => sg::UseKind::Read,
        crate::ast::UseKind::Unknown => sg::UseKind::Unknown,
    }
}

fn symbol_kind_for_binding(kind: BindingKind) -> SymbolKind {
    match kind {
        BindingKind::Import => SymbolKind::Import,
        BindingKind::Parameter => SymbolKind::Parameter,
        BindingKind::Class | BindingKind::Function | BindingKind::Method => SymbolKind::Callable,
        BindingKind::Field => SymbolKind::Field,
        BindingKind::External => SymbolKind::External,
        BindingKind::Assignment => SymbolKind::Local,
        BindingKind::Unknown => SymbolKind::Unknown,
    }
}

fn symbol_kind_for_use(kind: crate::ast::UseKind) -> SymbolKind {
    match kind {
        crate::ast::UseKind::Field => SymbolKind::Field,
        crate::ast::UseKind::Type => SymbolKind::Type,
        crate::ast::UseKind::Callee => SymbolKind::Callable,
        crate::ast::UseKind::Identifier => SymbolKind::Local,
        crate::ast::UseKind::Unknown => SymbolKind::Unknown,
    }
}

fn resolution_from_target(target: &BindingTarget) -> Resolution {
    match target {
        BindingTarget::Callable(_) | BindingTarget::Class(_) | BindingTarget::Module(_) => {
            Resolution::Exact
        }
        BindingTarget::External(_) => Resolution::External,
        BindingTarget::Value(_) => Resolution::Exact,
        BindingTarget::Unresolved(_) => Resolution::Unresolved,
    }
}

fn binding_confidence(target: &BindingTarget) -> Confidence {
    match target {
        BindingTarget::Unresolved(_) => Confidence::Unknown,
        BindingTarget::Value(_) => Confidence::Probable,
        _ => Confidence::Exact,
    }
}

fn confidence_from_resolution(resolution: Resolution) -> Confidence {
    match resolution {
        Resolution::Exact | Resolution::External => Confidence::Exact,
        Resolution::Probable | Resolution::Possible | Resolution::Ambiguous => Confidence::Probable,
        Resolution::Unresolved | Resolution::Unsupported => Confidence::Unknown,
    }
}

fn parser_evidence(
    semantic: &SemanticCallable<'_>,
    span: Option<SourceSpan>,
    syntax_kind: &str,
    summary: &str,
) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Parser,
        summary: summary.to_string(),
        source_id: Some(semantic.artifact().artifact_id.clone()),
        source_span: span,
        content_hash: None,
        syntax: Some(SyntaxReference {
            kind: syntax_kind.to_string(),
            node_key: span.map(|span| format!("{syntax_kind}:{}", span_key(span))),
            field_path: Vec::new(),
        }),
    }]
}

fn resolver_evidence(summary: &str, span: Option<SourceSpan>, syntax_kind: &str) -> Vec<Evidence> {
    vec![Evidence {
        kind: EvidenceKind::Resolver,
        summary: summary.to_string(),
        source_id: None,
        source_span: span,
        content_hash: None,
        syntax: Some(SyntaxReference {
            kind: syntax_kind.to_string(),
            node_key: span.map(|span| format!("{syntax_kind}:{}", span_key(span))),
            field_path: Vec::new(),
        }),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        AssignmentAst, CallAst, FieldAccessAst, FileAst, ImportAst, ImportNameAst, ParamAst,
        ProjectAst, StatementAst, StatementKind as AstStatementKind, SymbolAst,
        SymbolKind as AstSymbolKind, UseAst, UseKind,
    };
    use crate::supergraph::{CallGraphView, ScopeBindingBehavior};

    #[test]
    fn sg030_builds_python_lexical_symbol_tables() {
        let graph = build_python_supergraph(&python_project());

        assert_symbol(
            &graph,
            "LocalClient",
            SymbolKind::Import,
            Resolution::External,
        );
        assert_symbol(&graph, "items", SymbolKind::Parameter, Resolution::Exact);
        assert_symbol(&graph, "local", SymbolKind::Local, Resolution::Exact);
        assert_symbol(&graph, "field", SymbolKind::Field, Resolution::Exact);
        assert_symbol(&graph, "missing", SymbolKind::Local, Resolution::Unresolved);

        let local_symbol = symbol_named(&graph, "local", SymbolKind::Local);
        assert!(
            graph
                .indexes
                .symbol_to_definitions
                .contains_key(&local_symbol.symbol_id),
            "local symbol should be indexed to definitions"
        );

        let branch_binding = binding_named(&graph, "branch_value", BindingKind::Assignment);
        let branch_scope = scope(&graph, &branch_binding.scope_id);
        assert_ne!(
            branch_scope.binding_behavior,
            ScopeBindingBehavior::Transparent,
            "python transparent blocks should be walked through for bindings"
        );
    }

    #[test]
    fn sg030_honors_typescript_block_scopes_and_static_receivers() {
        let graph = build_typescript_supergraph(&typescript_project());

        assert_symbol(&graph, "Exported", SymbolKind::Callable, Resolution::Exact);
        assert_symbol(&graph, "item", SymbolKind::Parameter, Resolution::Exact);
        assert_symbol(&graph, "blockOnly", SymbolKind::Local, Resolution::Exact);
        assert_symbol(&graph, "prop", SymbolKind::Field, Resolution::Exact);
        assert_symbol(
            &graph,
            "unknownThing",
            SymbolKind::Local,
            Resolution::Unresolved,
        );

        let block_binding = binding_named(&graph, "blockOnly", BindingKind::Assignment);
        let block_scope = scope(&graph, &block_binding.scope_id);
        assert_eq!(
            block_scope.binding_behavior,
            ScopeBindingBehavior::Boundary,
            "TypeScript block bindings should stay in block scopes"
        );
    }

    #[test]
    fn sg031_resolves_python_symbols_uses_and_call_sites() {
        let graph = build_python_supergraph(&python_project());

        assert_symbol(
            &graph,
            "ambiguous",
            SymbolKind::Local,
            Resolution::Ambiguous,
        );
        assert_every_use_has_resolution(&graph);
        assert_every_call_site_has_resolution(&graph);

        let local_use = use_named(&graph, "local");
        let local_binding = binding_named(&graph, "local", BindingKind::Assignment);
        assert_resolution_edge(
            &graph,
            &local_use.use_id,
            &local_binding.binding_id,
            Resolution::Exact,
        );

        let field_use = use_named(&graph, "field");
        let field_binding = binding_named(&graph, "field", BindingKind::Field);
        assert_resolution_edge(
            &graph,
            &field_use.use_id,
            &field_binding.binding_id,
            Resolution::Exact,
        );

        let external_call = call_site_named(&graph, "LocalClient");
        assert_source_resolves_to_kind(
            &graph,
            &external_call.call_site_id,
            NodeKind::ExternalTarget,
            Resolution::External,
        );

        let dynamic_call = call_site_named(&graph, "local");
        assert_resolution_edge(
            &graph,
            &dynamic_call.call_site_id,
            &local_binding.binding_id,
            Resolution::Possible,
        );

        let unresolved_call = call_site_named(&graph, "missing_call");
        assert_source_resolves_to_kind(
            &graph,
            &unresolved_call.call_site_id,
            NodeKind::ExternalTarget,
            Resolution::Unresolved,
        );

        let ambiguous_use = use_named(&graph, "ambiguous");
        let ambiguous_edges = resolution_edges_from(&graph, &ambiguous_use.use_id)
            .into_iter()
            .filter(|edge| match &edge.fact {
                EdgeFact::ResolvesTo(resolves) => resolves.resolution == Resolution::Ambiguous,
                _ => false,
            })
            .count();
        assert!(
            ambiguous_edges >= 2,
            "ambiguous use should retain all candidate bindings"
        );
    }

    #[test]
    fn sg031_resolves_typescript_class_fields_and_dynamic_calls() {
        let graph = build_typescript_supergraph(&typescript_project());

        assert_every_use_has_resolution(&graph);
        assert_every_call_site_has_resolution(&graph);

        let method_call = call_site_named(&graph, "this.handle");
        assert_source_resolves_to_kind(
            &graph,
            &method_call.call_site_id,
            NodeKind::Callable,
            Resolution::Exact,
        );

        let block_binding = binding_named(&graph, "blockOnly", BindingKind::Assignment);
        let dynamic_call = call_site_named(&graph, "blockOnly");
        assert_resolution_edge(
            &graph,
            &dynamic_call.call_site_id,
            &block_binding.binding_id,
            Resolution::Possible,
        );

        let field_use = use_named(&graph, "prop");
        let field_binding = binding_named(&graph, "prop", BindingKind::Field);
        assert_resolution_edge(
            &graph,
            &field_use.use_id,
            &field_binding.binding_id,
            Resolution::Exact,
        );

        let unresolved_call = call_site_named(&graph, "unknownThing");
        assert_source_resolves_to_kind(
            &graph,
            &unresolved_call.call_site_id,
            NodeKind::ExternalTarget,
            Resolution::Unresolved,
        );
    }

    #[test]
    fn sg040_lowers_python_calls_from_call_site_resolutions() {
        let graph = build_python_supergraph(&python_project());

        assert_all_calls_are_call_site_sourced(&graph);

        let external_call = call_site_named(&graph, "LocalClient");
        assert_call_edge_to_kind(
            &graph,
            &external_call.call_site_id,
            NodeKind::ExternalTarget,
            Resolution::External,
        );

        let dynamic_call = call_site_named(&graph, "local");
        assert_targetless_call_edge(
            &graph,
            &dynamic_call.call_site_id,
            "local",
            Resolution::Possible,
        );

        let unresolved_call = call_site_named(&graph, "missing_call");
        assert_targetless_call_edge(
            &graph,
            &unresolved_call.call_site_id,
            "missing_call",
            Resolution::Unresolved,
        );

        let view = CallGraphView::new(&graph);
        assert!(
            view.calls_from_call_site(&dynamic_call.call_site_id)
                .iter()
                .any(|edge| edge.target_id.is_none()),
            "call graph view should answer call-site queries for possible dynamic calls"
        );
    }

    #[test]
    fn sg040_lowers_typescript_local_calls_and_keeps_caller_queries_compatible() {
        let graph = build_typescript_supergraph(&typescript_project());

        assert_all_calls_are_call_site_sourced(&graph);

        let method_call = call_site_named(&graph, "this.handle");
        let method_edges = call_edges_from(&graph, &method_call.call_site_id);
        let local_method_edge = method_edges
            .iter()
            .find(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::Calls(calls)
                        if calls.callee_callable_id.as_deref() == Some("sample.Exported.handle")
                            && calls.resolution == Resolution::Exact
                )
            })
            .expect("method call should lower to local callable Calls edge");
        assert_eq!(local_method_edge.source_id, method_call.call_site_id);
        assert_eq!(
            local_method_edge.target_id.as_deref(),
            Some("sample.Exported.handle")
        );

        let dynamic_call = call_site_named(&graph, "blockOnly");
        assert_targetless_call_edge(
            &graph,
            &dynamic_call.call_site_id,
            "blockOnly",
            Resolution::Possible,
        );

        let unresolved_call = call_site_named(&graph, "unknownThing");
        assert_targetless_call_edge(
            &graph,
            &unresolved_call.call_site_id,
            "unknownThing",
            Resolution::Unresolved,
        );

        let module_local_call = call_site_named(&graph, "initialize");
        assert_eq!(
            module_local_call.context,
            crate::ast::CallContext::ModuleInitializer
        );
        assert_eq!(
            module_local_call.enclosing_callable_id, "sample:<module>",
            "top-level TypeScript local calls should be owned by the module initializer"
        );
        assert_call_edge_to_target(
            &graph,
            &module_local_call.call_site_id,
            "sample.initialize",
            Resolution::Exact,
        );

        let module_external_call = call_site_named(&graph, "externalBoot");
        assert_eq!(
            module_external_call.enclosing_callable_id, "sample:<module>",
            "top-level TypeScript external calls should be owned by the module initializer"
        );
        assert_call_edge_to_kind(
            &graph,
            &module_external_call.call_site_id,
            NodeKind::ExternalTarget,
            Resolution::External,
        );

        let module_unresolved_call = call_site_named(&graph, "missingBoot");
        assert_eq!(
            module_unresolved_call.enclosing_callable_id, "sample:<module>",
            "top-level TypeScript unresolved calls should be owned by the module initializer"
        );
        assert_targetless_call_edge(
            &graph,
            &module_unresolved_call.call_site_id,
            "missingBoot",
            Resolution::Unresolved,
        );

        let view = CallGraphView::new(&graph);
        assert_eq!(
            view.calls_from_caller_to_callee("sample.Exported.handle", "sample.Exported.handle")
                .len(),
            1,
            "call graph view should answer caller-to-callee from canonical Calls edges"
        );

        assert!(
            graph
                .indexes
                .calls_by_caller
                .get("sample.Exported.handle")
                .is_some_and(|edges| edges.contains(&local_method_edge.edge_id)),
            "caller index should include the canonical self-call edge"
        );
        assert!(
            graph
                .indexes
                .calls_by_concrete_target
                .get("sample.Exported.handle")
                .is_some_and(|edges| edges.contains(&local_method_edge.edge_id)),
            "concrete-target index should include the canonical self-call edge"
        );
    }

    #[test]
    fn sg041_indexes_caller_to_target_summaries_without_legacy_call_edges() {
        let python_graph = build_python_supergraph(&python_project());
        assert_all_calls_are_call_site_sourced(&python_graph);

        let python_view = CallGraphView::new(&python_graph);
        let python_caller = "sample.Worker.run";

        let external_call = call_site_named(&python_graph, "LocalClient");
        let external_edge = call_edges_from(&python_graph, &external_call.call_site_id)
            .into_iter()
            .find(|edge| matches!(&edge.fact, EdgeFact::Calls(calls) if calls.resolution == Resolution::External))
            .expect("external call edge");
        let external_target = external_edge.target_id.as_deref().expect("external target");
        assert_caller_to_target_summary(
            &python_graph,
            &python_view,
            python_caller,
            external_target,
            &external_edge.edge_id,
        );

        let dynamic_call = call_site_named(&python_graph, "local");
        let dynamic_edge = call_edges_from(&python_graph, &dynamic_call.call_site_id)
            .into_iter()
            .find(|edge| {
                edge.target_id.is_none()
                    && matches!(
                        &edge.fact,
                        EdgeFact::Calls(calls)
                            if calls.resolution == Resolution::Possible
                                && calls.unresolved_target.as_deref() == Some("local")
                    )
            })
            .expect("possible dynamic call edge");
        assert_call_site_index_entry(
            &python_graph,
            &dynamic_call.call_site_id,
            &dynamic_edge.edge_id,
        );

        let unresolved_call = call_site_named(&python_graph, "missing_call");
        let unresolved_edge = call_edges_from(&python_graph, &unresolved_call.call_site_id)
            .into_iter()
            .find(|edge| {
                edge.target_id.is_none()
                    && matches!(
                        &edge.fact,
                        EdgeFact::Calls(calls)
                            if calls.resolution == Resolution::Unresolved
                                && calls.unresolved_target.as_deref() == Some("missing_call")
                    )
            })
            .expect("unresolved call edge");
        assert_call_site_index_entry(
            &python_graph,
            &unresolved_call.call_site_id,
            &unresolved_edge.edge_id,
        );

        let typescript_graph = build_typescript_supergraph(&typescript_project());
        assert_all_calls_are_call_site_sourced(&typescript_graph);

        let typescript_view = CallGraphView::new(&typescript_graph);
        let local_edges = typescript_view
            .calls_from_caller_to_callee("sample.Exported.handle", "sample.Exported.handle");
        assert_eq!(local_edges.len(), 1);
        assert_eq!(
            local_edges[0].source_id,
            call_site_named(&typescript_graph, "this.handle").call_site_id
        );
        assert!(
            typescript_view
                .call_targets_from_caller("sample.Exported.handle")
                .contains(&"sample.Exported.handle")
        );

        let module_local_call = call_site_named(&typescript_graph, "initialize");
        let module_local_edge = call_edges_from(&typescript_graph, &module_local_call.call_site_id)
            .into_iter()
            .find(|edge| edge.target_id.as_deref() == Some("sample.initialize"))
            .expect("module-initializer local call edge");
        assert_caller_to_target_summary(
            &typescript_graph,
            &typescript_view,
            "sample:<module>",
            "sample.initialize",
            &module_local_edge.edge_id,
        );

        let canonical_call_count = typescript_graph
            .edges
            .iter()
            .filter(|edge| matches!(edge.fact, EdgeFact::Calls(_)))
            .count();
        let call_site_index_count = typescript_graph
            .indexes
            .call_site_to_calls
            .values()
            .map(Vec::len)
            .sum::<usize>();
        assert_eq!(
            call_site_index_count, canonical_call_count,
            "call-site index should cover canonical Calls edges without summary duplicates"
        );
        for edge in typescript_graph
            .edges
            .iter()
            .filter(|edge| matches!(edge.fact, EdgeFact::Calls(_)))
        {
            assert!(
                typescript_graph
                    .indexes
                    .call_site_to_calls
                    .values()
                    .any(|edges| edges.contains(&edge.edge_id)),
                "call-site index should include canonical Calls edge {}",
                edge.edge_id
            );
        }
    }

    fn assert_symbol(
        graph: &ProgramSupergraph,
        name: &str,
        kind: SymbolKind,
        resolution: Resolution,
    ) {
        assert!(
            graph.nodes.iter().any(|node| matches!(
                &node.fact,
                NodeFact::Symbol(symbol)
                    if symbol.name == name
                        && symbol.kind == kind
                        && symbol.resolution == resolution
            )),
            "missing symbol {name} {kind:?} {resolution:?}"
        );
    }

    fn symbol_named<'a>(
        graph: &'a ProgramSupergraph,
        name: &str,
        kind: SymbolKind,
    ) -> &'a sg::Symbol {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Symbol(symbol) if symbol.name == name && symbol.kind == kind => {
                    Some(symbol)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing symbol {name} {kind:?}"))
    }

    fn binding_named<'a>(
        graph: &'a ProgramSupergraph,
        name: &str,
        kind: BindingKind,
    ) -> &'a sg::Binding {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Binding(binding) if binding.name == name && binding.kind == kind => {
                    Some(binding)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing binding {name} {kind:?}"))
    }

    fn use_named<'a>(graph: &'a ProgramSupergraph, name: &str) -> &'a sg::Use {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Use(use_fact) if use_fact.name.as_deref() == Some(name) => Some(use_fact),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing use {name}"))
    }

    fn call_site_named<'a>(graph: &'a ProgramSupergraph, callee: &str) -> &'a sg::CallSite {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::CallSite(call_site) if call_site.callee_expression == callee => {
                    Some(call_site)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing call site {callee}"))
    }

    fn call_edges_from<'a>(
        graph: &'a ProgramSupergraph,
        call_site_id: &str,
    ) -> Vec<&'a sg::GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::Calls(calls) if calls.call_site_id == call_site_id
                )
            })
            .collect()
    }

    fn assert_all_calls_are_call_site_sourced(graph: &ProgramSupergraph) {
        for edge in &graph.edges {
            let EdgeFact::Calls(calls) = &edge.fact else {
                continue;
            };
            assert_eq!(
                edge.source_id, calls.call_site_id,
                "Calls edge {} should be sourced from its call site",
                edge.edge_id
            );
            let expected_target = calls
                .callee_callable_id
                .as_ref()
                .or(calls.external_target_id.as_ref());
            assert_eq!(
                edge.target_id.as_ref(),
                expected_target,
                "Calls edge {} target should match its concrete call payload target",
                edge.edge_id
            );
            if calls.resolution == Resolution::Unresolved {
                assert!(
                    edge.target_id.is_none(),
                    "unresolved Calls edge {} should be targetless",
                    edge.edge_id
                );
                assert!(
                    calls.unresolved_target.is_some(),
                    "unresolved Calls edge {} should retain unresolved target text",
                    edge.edge_id
                );
            }
        }
    }

    fn assert_call_edge_to_target(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        target_id: &str,
        resolution: Resolution,
    ) {
        assert!(
            call_edges_from(graph, call_site_id)
                .into_iter()
                .any(|edge| {
                    edge.target_id.as_deref() == Some(target_id)
                        && matches!(
                            &edge.fact,
                            EdgeFact::Calls(calls) if calls.resolution == resolution
                        )
                }),
            "missing {resolution:?} Calls edge from {call_site_id} to {target_id}"
        );
    }

    fn assert_call_edge_to_kind(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        target_kind: NodeKind,
        resolution: Resolution,
    ) {
        assert!(
            call_edges_from(graph, call_site_id)
                .into_iter()
                .any(|edge| {
                    let Some(target_id) = edge.target_id.as_deref() else {
                        return false;
                    };
                    let target_matches = graph
                        .nodes
                        .iter()
                        .any(|node| node.node_id == target_id && node.kind == target_kind);
                    target_matches
                        && matches!(
                            &edge.fact,
                            EdgeFact::Calls(calls) if calls.resolution == resolution
                        )
                }),
            "missing {resolution:?} Calls edge from {call_site_id} to {target_kind:?}"
        );
    }

    fn assert_targetless_call_edge(
        graph: &ProgramSupergraph,
        call_site_id: &str,
        unresolved_target: &str,
        resolution: Resolution,
    ) {
        assert!(
            call_edges_from(graph, call_site_id)
                .into_iter()
                .any(|edge| {
                    edge.target_id.is_none()
                        && matches!(
                            &edge.fact,
                            EdgeFact::Calls(calls)
                                if calls.resolution == resolution
                                    && calls.callee_callable_id.is_none()
                                    && calls.external_target_id.is_none()
                                    && calls.unresolved_target.as_deref()
                                        == Some(unresolved_target)
                        )
                }),
            "missing targetless {resolution:?} Calls edge from {call_site_id} for {unresolved_target}"
        );
    }

    fn assert_caller_to_target_summary(
        graph: &ProgramSupergraph,
        view: &CallGraphView<'_>,
        caller_id: &str,
        target_id: &str,
        edge_id: &str,
    ) {
        assert!(
            graph
                .indexes
                .caller_to_concrete_target_calls
                .get(caller_id)
                .and_then(|targets| targets.get(target_id))
                .is_some_and(|edges| edges.contains(&edge_id.to_string())),
            "missing caller-to-concrete-target summary index for {caller_id} -> {target_id}"
        );
        assert!(
            graph
                .indexes
                .caller_to_concrete_call_targets
                .get(caller_id)
                .is_some_and(|targets| targets.contains(&target_id.to_string())),
            "missing caller-to-concrete-target target-list index for {caller_id} -> {target_id}"
        );
        let edges = view.calls_from_caller_to_target(caller_id, target_id);
        assert!(
            edges.iter().any(|edge| edge.edge_id == edge_id),
            "view should return canonical call edge {edge_id} for {caller_id} -> {target_id}"
        );
        assert!(
            edges
                .iter()
                .all(|edge| matches!(&edge.fact, EdgeFact::Calls(calls) if edge.source_id == calls.call_site_id)),
            "summary traversal should return canonical call-site Calls edges"
        );
    }

    fn assert_call_site_index_entry(graph: &ProgramSupergraph, call_site_id: &str, edge_id: &str) {
        assert!(
            graph
                .indexes
                .call_site_to_calls
                .get(call_site_id)
                .is_some_and(|edges| edges.contains(&edge_id.to_string())),
            "missing call-site index entry for {call_site_id} -> {edge_id}"
        );
    }

    fn resolution_edges_from<'a>(
        graph: &'a ProgramSupergraph,
        source_id: &str,
    ) -> Vec<&'a sg::GraphEdge> {
        graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::ResolvesTo && edge.source_id == source_id)
            .collect()
    }

    fn assert_resolution_edge(
        graph: &ProgramSupergraph,
        source_id: &str,
        target_id: &str,
        resolution: Resolution,
    ) {
        assert!(
            graph.edges.iter().any(|edge| matches!(
                &edge.fact,
                EdgeFact::ResolvesTo(resolves)
                    if edge.source_id == source_id
                        && edge.target_id.as_deref() == Some(target_id)
                        && resolves.resolution == resolution
            )),
            "missing {resolution:?} ResolvesTo edge from {source_id} to {target_id}"
        );
    }

    fn assert_source_resolves_to_kind(
        graph: &ProgramSupergraph,
        source_id: &str,
        target_kind: NodeKind,
        resolution: Resolution,
    ) {
        assert!(
            resolution_edges_from(graph, source_id)
                .into_iter()
                .any(|edge| {
                    let Some(target_id) = edge.target_id.as_deref() else {
                        return false;
                    };
                    let target_matches = graph
                        .nodes
                        .iter()
                        .any(|node| node.node_id == target_id && node.kind == target_kind);
                    matches!(
                        &edge.fact,
                        EdgeFact::ResolvesTo(resolves)
                            if resolves.resolution == resolution && target_matches
                    )
                }),
            "missing {resolution:?} ResolvesTo edge from {source_id} to {target_kind:?}"
        );
    }

    fn assert_every_use_has_resolution(graph: &ProgramSupergraph) {
        for node in &graph.nodes {
            if let NodeFact::Use(use_fact) = &node.fact {
                assert!(
                    !resolution_edges_from(graph, &use_fact.use_id).is_empty(),
                    "use {} should have at least one ResolvesTo edge",
                    use_fact.use_id
                );
            }
        }
    }

    fn assert_every_call_site_has_resolution(graph: &ProgramSupergraph) {
        for node in &graph.nodes {
            if let NodeFact::CallSite(call_site) = &node.fact {
                assert!(
                    !resolution_edges_from(graph, &call_site.call_site_id).is_empty(),
                    "call site {} should have at least one ResolvesTo edge",
                    call_site.call_site_id
                );
            }
        }
    }

    fn scope<'a>(graph: &'a ProgramSupergraph, scope_id: &str) -> &'a sg::Scope {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Scope(scope) if scope.scope_id == scope_id => Some(scope),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing scope {scope_id}"))
    }

    fn python_project() -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: "sample.py".to_string(),
                imports: vec![ImportAst {
                    text: "from vendor import Client as LocalClient".to_string(),
                    module: Some("vendor".to_string()),
                    names: vec![ImportNameAst {
                        name: "Client".to_string(),
                        alias: Some("LocalClient".to_string()),
                    }],
                    source_span: span(0, 35),
                }],
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![
                    SymbolAst {
                        id: "sample:Worker".to_string(),
                        name: "Worker".to_string(),
                        kind: AstSymbolKind::Class,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: Vec::new(),
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(40, 220)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: Vec::new(),
                        statements: Vec::new(),
                        expressions: Vec::new(),
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(36, 230),
                    },
                    SymbolAst {
                        id: "sample:Worker.run".to_string(),
                        name: "run".to_string(),
                        kind: AstSymbolKind::Method,
                        module_path: "sample".to_string(),
                        parent: Some("Worker".to_string()),
                        parameters: vec![
                            ParamAst {
                                name: "self".to_string(),
                                text: "self".to_string(),
                                source_span: span(55, 59),
                            },
                            ParamAst {
                                name: "items".to_string(),
                                text: "items".to_string(),
                                source_span: span(61, 66),
                            },
                        ],
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(70, 220)),
                        assignments: vec![AssignmentAst {
                            target: "local".to_string(),
                            value: Some("LocalClient()".to_string()),
                            text: "local = LocalClient()".to_string(),
                            source_span: span(75, 95),
                        }],
                        calls: vec![
                            call("LocalClient", None, 82, 95),
                            call("local", None, 188, 195),
                            call("missing_call", None, 200, 212),
                        ],
                        raises: Vec::new(),
                        statements: vec![
                            statement(
                                AstStatementKind::Assignment,
                                "local = LocalClient()",
                                "sample:Worker.run",
                                span(75, 95),
                            ),
                            statement(
                                AstStatementKind::If,
                                "if items",
                                "sample:Worker.run",
                                span(100, 145),
                            ),
                            statement(
                                AstStatementKind::Assignment,
                                "branch_value = 1",
                                "sample:Worker.run",
                                span(110, 130),
                            ),
                            statement(
                                AstStatementKind::Assignment,
                                "self.field = local",
                                "sample:Worker.run",
                                span(150, 170),
                            ),
                        ],
                        expressions: Vec::new(),
                        conditions: Vec::new(),
                        definitions: vec![
                            definition(
                                "self",
                                AstDefinitionKind::Parameter,
                                "self",
                                "sample:Worker.run",
                                span(55, 59),
                            ),
                            definition(
                                "items",
                                AstDefinitionKind::Parameter,
                                "items",
                                "sample:Worker.run",
                                span(61, 66),
                            ),
                            definition(
                                "local",
                                AstDefinitionKind::Variable,
                                "local",
                                "sample:Worker.run",
                                span(75, 80),
                            ),
                            definition(
                                "branch_value",
                                AstDefinitionKind::Variable,
                                "branch_value",
                                "sample:Worker.run",
                                span(110, 122),
                            ),
                            definition(
                                "field",
                                AstDefinitionKind::Field,
                                "self.field",
                                "sample:Worker.run",
                                span(150, 160),
                            ),
                            definition(
                                "ambiguous",
                                AstDefinitionKind::Variable,
                                "ambiguous",
                                "sample:Worker.run",
                                span(172, 181),
                            ),
                            definition(
                                "ambiguous",
                                AstDefinitionKind::Unknown,
                                "ambiguous",
                                "sample:Worker.run",
                                span(182, 191),
                            ),
                        ],
                        uses: vec![
                            use_fact(
                                "LocalClient",
                                UseKind::Callee,
                                "sample:Worker.run",
                                span(82, 93),
                            ),
                            use_fact(
                                "items",
                                UseKind::Identifier,
                                "sample:Worker.run",
                                span(103, 108),
                            ),
                            use_fact(
                                "local",
                                UseKind::Identifier,
                                "sample:Worker.run",
                                span(165, 170),
                            ),
                            use_fact(
                                "missing",
                                UseKind::Identifier,
                                "sample:Worker.run",
                                span(180, 187),
                            ),
                            use_fact(
                                "ambiguous",
                                UseKind::Identifier,
                                "sample:Worker.run",
                                span(208, 217),
                            ),
                        ],
                        returns: Vec::new(),
                        field_accesses: vec![FieldAccessAst {
                            object: Some("self".to_string()),
                            field: "field".to_string(),
                            text: "self.field".to_string(),
                            owner_id: "sample:Worker.run".to_string(),
                            source_span: span(150, 160),
                        }],
                        index_accesses: Vec::new(),
                        source_span: span(50, 220),
                    },
                ],
                statements: vec![statement(
                    AstStatementKind::Class,
                    "class Worker",
                    "sample:<module>",
                    span(36, 230),
                )],
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

    fn typescript_project() -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: "sample.ts".to_string(),
                imports: vec![ImportAst {
                    text: r#"import { externalBoot } from "pkg";"#.to_string(),
                    module: Some("pkg".to_string()),
                    names: vec![ImportNameAst {
                        name: "externalBoot".to_string(),
                        alias: None,
                    }],
                    source_span: span(0, 35),
                }],
                assignments: Vec::new(),
                calls: vec![
                    module_call("initialize", None, 212, 224),
                    module_call("externalBoot", None, 226, 242),
                    module_call("missingBoot", None, 244, 257),
                ],
                symbols: vec![
                    SymbolAst {
                        id: "sample:Exported".to_string(),
                        name: "Exported".to_string(),
                        kind: AstSymbolKind::Class,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: Vec::new(),
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(0, 200)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: Vec::new(),
                        statements: Vec::new(),
                        expressions: Vec::new(),
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(0, 210),
                    },
                    SymbolAst {
                        id: "sample:Exported.handle".to_string(),
                        name: "handle".to_string(),
                        kind: AstSymbolKind::Method,
                        module_path: "sample".to_string(),
                        parent: Some("Exported".to_string()),
                        parameters: vec![
                            ParamAst {
                                name: "this".to_string(),
                                text: "this".to_string(),
                                source_span: span(25, 29),
                            },
                            ParamAst {
                                name: "item".to_string(),
                                text: "item: string".to_string(),
                                source_span: span(31, 43),
                            },
                        ],
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(45, 190)),
                        assignments: Vec::new(),
                        calls: vec![
                            call("blockOnly", None, 84, 92),
                            call("this.handle", Some("this"), 134, 145),
                            call("unknownThing", None, 150, 162),
                        ],
                        raises: Vec::new(),
                        statements: vec![
                            statement(
                                AstStatementKind::If,
                                "if (item)",
                                "sample:Exported.handle",
                                span(60, 100),
                            ),
                            statement(
                                AstStatementKind::Assignment,
                                "let blockOnly = item",
                                "sample:Exported.handle",
                                span(70, 90),
                            ),
                            statement(
                                AstStatementKind::Assignment,
                                "this.prop = blockOnly",
                                "sample:Exported.handle",
                                span(110, 132),
                            ),
                        ],
                        expressions: Vec::new(),
                        conditions: Vec::new(),
                        definitions: vec![
                            definition(
                                "this",
                                AstDefinitionKind::Parameter,
                                "this",
                                "sample:Exported.handle",
                                span(25, 29),
                            ),
                            definition(
                                "item",
                                AstDefinitionKind::Parameter,
                                "item: string",
                                "sample:Exported.handle",
                                span(31, 43),
                            ),
                            definition(
                                "blockOnly",
                                AstDefinitionKind::Variable,
                                "blockOnly",
                                "sample:Exported.handle",
                                span(74, 83),
                            ),
                            definition(
                                "prop",
                                AstDefinitionKind::Field,
                                "this.prop",
                                "sample:Exported.handle",
                                span(110, 119),
                            ),
                        ],
                        uses: vec![
                            use_fact(
                                "item",
                                UseKind::Identifier,
                                "sample:Exported.handle",
                                span(64, 68),
                            ),
                            use_fact(
                                "blockOnly",
                                UseKind::Identifier,
                                "sample:Exported.handle",
                                span(122, 131),
                            ),
                            use_fact(
                                "unknownThing",
                                UseKind::Identifier,
                                "sample:Exported.handle",
                                span(150, 162),
                            ),
                        ],
                        returns: Vec::new(),
                        field_accesses: vec![FieldAccessAst {
                            object: Some("this".to_string()),
                            field: "prop".to_string(),
                            text: "this.prop".to_string(),
                            owner_id: "sample:Exported.handle".to_string(),
                            source_span: span(110, 119),
                        }],
                        index_accesses: Vec::new(),
                        source_span: span(20, 190),
                    },
                    SymbolAst {
                        id: "sample:initialize".to_string(),
                        name: "initialize".to_string(),
                        kind: AstSymbolKind::Function,
                        module_path: "sample".to_string(),
                        parent: None,
                        parameters: Vec::new(),
                        decorators: Vec::new(),
                        return_type: None,
                        body_span: Some(span(260, 280)),
                        assignments: Vec::new(),
                        calls: Vec::new(),
                        raises: Vec::new(),
                        statements: Vec::new(),
                        expressions: Vec::new(),
                        conditions: Vec::new(),
                        definitions: Vec::new(),
                        uses: Vec::new(),
                        returns: Vec::new(),
                        field_accesses: Vec::new(),
                        index_accesses: Vec::new(),
                        source_span: span(258, 282),
                    },
                ],
                statements: vec![
                    statement(
                        AstStatementKind::Class,
                        "export class Exported",
                        "sample:<module>",
                        span(0, 210),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "initialize()",
                        "sample:<module>",
                        span(212, 225),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "externalBoot()",
                        "sample:<module>",
                        span(226, 243),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "missingBoot()",
                        "sample:<module>",
                        span(244, 258),
                    ),
                    statement(
                        AstStatementKind::Function,
                        "function initialize() {}",
                        "sample:<module>",
                        span(258, 282),
                    ),
                ],
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: vec![definition(
                    "initialize",
                    AstDefinitionKind::Function,
                    "initialize",
                    "sample:<module>",
                    span(267, 277),
                )],
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn definition(
        name: &str,
        kind: AstDefinitionKind,
        text: &str,
        owner_id: &str,
        source_span: SourceSpan,
    ) -> DefinitionAst {
        DefinitionAst {
            name: name.to_string(),
            kind,
            text: text.to_string(),
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn use_fact(name: &str, kind: UseKind, owner_id: &str, source_span: SourceSpan) -> UseAst {
        UseAst {
            name: name.to_string(),
            kind,
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn call(callee: &str, receiver: Option<&str>, start_byte: usize, end_byte: usize) -> CallAst {
        CallAst {
            callee: callee.to_string(),
            receiver: receiver.map(str::to_string),
            argument_names: Vec::new(),
            args_count: 0,
            context: crate::ast::CallContext::Body,
            source_span: span(start_byte, end_byte),
        }
    }

    fn module_call(
        callee: &str,
        receiver: Option<&str>,
        start_byte: usize,
        end_byte: usize,
    ) -> CallAst {
        CallAst {
            callee: callee.to_string(),
            receiver: receiver.map(str::to_string),
            argument_names: Vec::new(),
            args_count: 0,
            context: crate::ast::CallContext::ModuleInitializer,
            source_span: span(start_byte, end_byte),
        }
    }

    fn statement(
        kind: AstStatementKind,
        text: &str,
        owner_id: &str,
        source_span: SourceSpan,
    ) -> StatementAst {
        StatementAst {
            kind,
            text: text.to_string(),
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn span(start_byte: usize, end_byte: usize) -> SourceSpan {
        SourceSpan {
            start_byte,
            end_byte,
            start_row: start_byte,
            start_column: 0,
            end_row: end_byte,
            end_column: 0,
        }
    }
}
