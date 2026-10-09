use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use tree_sitter::{Node, Parser, Tree};

use crate::ast::{
    AssignmentAst, CallAst, CallContext, ConditionAst, ConditionKind, DecoratorAst, DefinitionAst,
    DefinitionKind, ExpressionAst, ExpressionKind, FieldAccessAst, FileAst, ImportAst,
    ImportNameAst, IndexAccessAst, ParamAst, RaiseAst, ReturnAst, SourceSpan, StatementAst,
    StatementKind, SymbolAst, SymbolKind, UseAst, UseKind,
};

#[derive(Default)]
struct SemanticFacts {
    statements: Vec<StatementAst>,
    expressions: Vec<ExpressionAst>,
    conditions: Vec<ConditionAst>,
    definitions: Vec<DefinitionAst>,
    uses: Vec<UseAst>,
    returns: Vec<ReturnAst>,
    raises: Vec<RaiseAst>,
    field_accesses: Vec<FieldAccessAst>,
    index_accesses: Vec<IndexAccessAst>,
}

pub struct ParsedRustFile {
    pub relative_path: String,
    source: String,
    tree: Tree,
}

pub fn parse_rust_file(path: &Path, relative_path: String) -> Result<ParsedRustFile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read Rust source {}", path.display()))?;
    parse_rust_tree(relative_path, source)
}

fn parse_rust_tree(relative_path: String, source: String) -> Result<ParsedRustFile> {
    let mut parser = Parser::new();
    let language = tree_sitter_rust::LANGUAGE.into();
    parser
        .set_language(&language)
        .context("failed to configure Tree-sitter Rust parser")?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow!("Tree-sitter failed to parse {}", relative_path))?;
    Ok(ParsedRustFile {
        relative_path,
        source,
        tree,
    })
}

pub fn filter_file(parsed: &ParsedRustFile) -> FileAst {
    let root = parsed.tree.root_node();
    let module_path = module_path(&parsed.relative_path);
    let mut imports = Vec::new();
    let mut assignments = Vec::new();
    let mut calls = Vec::new();
    let mut symbols = Vec::new();

    for child in named_children(root) {
        collect_top_level(
            child,
            &parsed.source,
            &module_path,
            &mut imports,
            &mut assignments,
            &mut calls,
            &mut symbols,
        );
    }

    let module_owner = module_owner_id(&module_path);
    let mut facts = semantic_facts_in_scope(root, &parsed.source, &module_owner);
    facts.definitions.extend(
        imports
            .iter()
            .flat_map(|import| definitions_for_import(import, &module_owner)),
    );
    for symbol in &symbols {
        facts.definitions.push(DefinitionAst {
            name: qualified_symbol_name(symbol),
            kind: match symbol.kind {
                SymbolKind::Class => DefinitionKind::Class,
                SymbolKind::Function | SymbolKind::Method => DefinitionKind::Function,
            },
            text: symbol.name.clone(),
            owner_id: symbol
                .parent
                .as_ref()
                .map(|parent| format!("{module_path}:{parent}"))
                .unwrap_or_else(|| module_owner.clone()),
            source_span: symbol.source_span,
        });
        facts.extend_symbol(symbol);
    }

    FileAst {
        path: parsed.relative_path.clone(),
        imports,
        assignments,
        calls,
        symbols,
        statements: facts.statements,
        expressions: facts.expressions,
        conditions: facts.conditions,
        definitions: facts.definitions,
        uses: facts.uses,
        returns: facts.returns,
        raises: facts.raises,
        field_accesses: facts.field_accesses,
        index_accesses: facts.index_accesses,
        parse_errors: super::collect_parse_errors(root, &parsed.source),
    }
}

fn collect_top_level(
    node: Node,
    source: &str,
    module_path: &str,
    imports: &mut Vec<ImportAst>,
    assignments: &mut Vec<AssignmentAst>,
    calls: &mut Vec<CallAst>,
    symbols: &mut Vec<SymbolAst>,
) {
    match node.kind() {
        "use_declaration" => imports.extend(import_asts(node, source)),
        "mod_item" => {
            // Inline modules are flattened into the enclosing file's module.
            if let Some(body) = node.child_by_field_name("body") {
                for child in named_children(body) {
                    collect_top_level(
                        child,
                        source,
                        module_path,
                        imports,
                        assignments,
                        calls,
                        symbols,
                    );
                }
            }
        }
        "const_item" | "static_item" => {
            if let Some(name) = node.child_by_field_name("name") {
                assignments.push(AssignmentAst {
                    target: text(name, source),
                    value: node
                        .child_by_field_name("value")
                        .map(|value| text(value, source)),
                    text: text(node, source),
                    source_span: span(node),
                });
            }
            calls.extend(top_level_calls_in(node, source));
        }
        "function_item" => collect_function(node, source, module_path, None, symbols),
        "struct_item" | "enum_item" | "union_item" | "trait_item" => {
            collect_class(node, source, module_path, symbols);
        }
        "impl_item" => collect_impl(node, source, module_path, symbols),
        "type_item" => {
            if let Some(name) = node.child_by_field_name("name") {
                assignments.push(AssignmentAst {
                    target: text(name, source),
                    value: None,
                    text: text(node, source),
                    source_span: span(node),
                });
            }
        }
        "attribute_item" | "inner_attribute_item" | "line_comment" | "block_comment" => {}
        _ => calls.extend(top_level_calls_in(node, source)),
    }
}

fn collect_class(node: Node, source: &str, module_path: &str, symbols: &mut Vec<SymbolAst>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text(name_node, source);
    let id = format!("{module_path}:{name}");

    symbols.push(SymbolAst {
        id,
        name: name.clone(),
        kind: SymbolKind::Class,
        module_path: module_path.to_string(),
        parent: None,
        parameters: Vec::new(),
        decorators: decorators_in(node, source),
        return_type: None,
        body_span: node.child_by_field_name("body").map(span),
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
        source_span: span(node),
    });

    // Default method bodies declared in a trait belong to the trait.
    if node.kind() == "trait_item"
        && let Some(body) = node.child_by_field_name("body")
    {
        for child in named_children(body) {
            if child.kind() == "function_item" {
                collect_function(child, source, module_path, Some(&name), symbols);
            }
        }
    }
}

fn collect_impl(node: Node, source: &str, module_path: &str, symbols: &mut Vec<SymbolAst>) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let type_name = type_name(type_node, source);
    for child in named_children(body) {
        if child.kind() == "function_item" {
            collect_function(child, source, module_path, Some(&type_name), symbols);
        }
    }
}

fn collect_function(
    node: Node,
    source: &str,
    module_path: &str,
    parent: Option<&str>,
    symbols: &mut Vec<SymbolAst>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text(name_node, source);
    let qualified_name = parent
        .map(|parent| format!("{parent}.{name}"))
        .unwrap_or_else(|| name.clone());
    let id = format!("{module_path}:{qualified_name}");
    let body = node.child_by_field_name("body");
    let parameters = node
        .child_by_field_name("parameters")
        .map(|parameters| params_in(parameters, source))
        .unwrap_or_default();
    let mut facts = body
        .map(|body| semantic_facts_in_scope(body, source, &id))
        .unwrap_or_default();
    if let Some(tail) = body.and_then(|body| tail_value_expression(body, source)) {
        add_tail_return(&mut facts, tail, source, &id);
    }
    facts.definitions.extend(
        parameters
            .iter()
            .map(|parameter| definition_for_param(parameter, &id)),
    );

    symbols.push(SymbolAst {
        id,
        name,
        kind: if parent.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        },
        module_path: module_path.to_string(),
        parent: parent.map(str::to_string),
        parameters,
        decorators: decorators_in(node, source),
        return_type: node
            .child_by_field_name("return_type")
            .map(|return_type| text(return_type, source)),
        body_span: body.map(span),
        assignments: body
            .map(|body| assignments_in(body, source))
            .unwrap_or_default(),
        calls: calls_in(node, source, CallContext::Body),
        raises: facts.raises.clone(),
        statements: facts.statements,
        expressions: facts.expressions,
        conditions: facts.conditions,
        definitions: facts.definitions,
        uses: facts.uses,
        returns: facts.returns,
        field_accesses: facts.field_accesses,
        index_accesses: facts.index_accesses,
        source_span: span(node),
    });
}

/// The unterminated final expression of a block is its value, which Rust
/// returns implicitly. Control-flow expressions are excluded because their own
/// statement facts already describe them.
fn tail_value_expression<'tree>(block: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    let last = named_children(block)
        .into_iter()
        .rev()
        .find(|child| !is_comment(*child))?;
    let is_value = !matches!(
        last.kind(),
        "expression_statement"
            | "let_declaration"
            | "empty_statement"
            | "attribute_item"
            | "function_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "use_declaration"
            | "const_item"
            | "static_item"
            | "type_item"
            | "macro_definition"
            | "block"
            | "unsafe_block"
    ) && !is_control_flow_expression(last)
        && !is_panic_macro(last, source);
    is_value.then_some(last)
}

fn add_tail_return(facts: &mut SemanticFacts, tail: Node, source: &str, owner_id: &str) {
    let tail_span = span(tail);
    let position = facts
        .statements
        .iter()
        .position(|statement| statement.source_span.start_byte >= tail_span.start_byte)
        .unwrap_or(facts.statements.len());
    facts.statements.insert(
        position,
        StatementAst {
            kind: StatementKind::Return,
            text: text(tail, source),
            owner_id: owner_id.to_string(),
            source_span: tail_span,
        },
    );
    facts.returns.push(ReturnAst {
        value: Some(text(tail, source)),
        owner_id: owner_id.to_string(),
        source_span: tail_span,
    });
}

enum UseEntry {
    Name {
        path: Vec<String>,
        alias: Option<String>,
    },
    Glob(Vec<String>),
}

fn import_asts(node: Node, source: &str) -> Vec<ImportAst> {
    let import_text = text(node, source);
    let Some(argument) = node.child_by_field_name("argument") else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    flatten_use(argument, &[], source, &mut entries);

    entries
        .into_iter()
        .map(|entry| {
            let (module, names) = match entry {
                UseEntry::Glob(path) => {
                    (Some(path.join(".")).filter(|module| !module.is_empty()), Vec::new())
                }
                UseEntry::Name { mut path, alias } => {
                    // `use a::b::{self}` imports the module `a::b` itself.
                    if path.len() > 1 && path.last().map(String::as_str) == Some("self") {
                        path.pop();
                    }
                    let name = path.pop().unwrap_or_default();
                    let module = (!path.is_empty()).then(|| path.join("."));
                    (module, vec![ImportNameAst { name, alias }])
                }
            };
            ImportAst {
                text: import_text.clone(),
                module,
                names,
                source_span: span(node),
            }
        })
        .collect()
}

fn path_segments(node: Node, source: &str) -> Vec<String> {
    text(node, source)
        .split("::")
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn flatten_use(node: Node, prefix: &[String], source: &str, out: &mut Vec<UseEntry>) {
    let with_prefix = |segments: Vec<String>| {
        let mut path = prefix.to_vec();
        path.extend(segments);
        path
    };
    match node.kind() {
        "use_list" => {
            for child in named_children(node) {
                flatten_use(child, prefix, source, out);
            }
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .map(|path| path_segments(path, source))
                .unwrap_or_default();
            let prefix = with_prefix(path);
            if let Some(list) = node.child_by_field_name("list") {
                flatten_use(list, &prefix, source, out);
            }
        }
        "use_as_clause" => {
            let path = node
                .child_by_field_name("path")
                .map(|path| path_segments(path, source))
                .unwrap_or_default();
            let alias = node
                .child_by_field_name("alias")
                .map(|alias| text(alias, source));
            out.push(UseEntry::Name {
                path: with_prefix(path),
                alias,
            });
        }
        "use_wildcard" => {
            let path = named_children(node)
                .into_iter()
                .next()
                .map(|path| path_segments(path, source))
                .unwrap_or_default();
            out.push(UseEntry::Glob(with_prefix(path)));
        }
        _ => out.push(UseEntry::Name {
            path: with_prefix(path_segments(node, source)),
            alias: None,
        }),
    }
}

fn assignments_in(node: Node, source: &str) -> Vec<AssignmentAst> {
    let mut assignments = Vec::new();
    visit_named_descendants(node, &mut |descendant| {
        if descendant.kind() != "let_declaration" {
            return;
        }
        let Some(pattern) = descendant.child_by_field_name("pattern") else {
            return;
        };
        assignments.push(AssignmentAst {
            target: binding_name(pattern, source),
            value: descendant
                .child_by_field_name("value")
                .map(|value| text(value, source)),
            text: text(descendant, source),
            source_span: span(descendant),
        });
    });
    assignments
}

fn params_in(node: Node, source: &str) -> Vec<ParamAst> {
    named_children(node)
        .into_iter()
        .filter_map(|child| match child.kind() {
            "parameter" => {
                let pattern = child.child_by_field_name("pattern")?;
                Some(ParamAst {
                    name: binding_name(pattern, source),
                    text: text(child, source),
                    source_span: span(child),
                })
            }
            "self_parameter" => Some(ParamAst {
                name: "self".to_string(),
                text: text(child, source),
                source_span: span(child),
            }),
            _ => None,
        })
        .collect()
}

/// Outer attributes such as `#[derive(Debug)]` are the closest Rust analogue
/// of decorators; they are siblings that immediately precede the item.
fn decorators_in(node: Node, source: &str) -> Vec<DecoratorAst> {
    let mut decorators = Vec::new();
    let mut current = node.prev_named_sibling();
    while let Some(sibling) = current {
        match sibling.kind() {
            "attribute_item" => decorators.push(DecoratorAst {
                text: text(sibling, source)
                    .trim_start_matches("#[")
                    .trim_end_matches(']')
                    .trim()
                    .to_string(),
                source_span: span(sibling),
            }),
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        current = sibling.prev_named_sibling();
    }
    decorators.reverse();
    decorators
}

fn calls_in(node: Node, source: &str, context: CallContext) -> Vec<CallAst> {
    let mut calls = Vec::new();
    visit_named_descendants(node, &mut |descendant| {
        push_call_if_supported(descendant, source, context, &mut calls);
    });
    calls
}

fn top_level_calls_in(node: Node, source: &str) -> Vec<CallAst> {
    let mut calls = Vec::new();
    visit_top_level_call_descendants(node, &mut |descendant| {
        push_call_if_supported(
            descendant,
            source,
            CallContext::ModuleInitializer,
            &mut calls,
        );
    });
    calls
}

fn push_call_if_supported(
    node: Node,
    source: &str,
    context: CallContext,
    calls: &mut Vec<CallAst>,
) {
    match node.kind() {
        "call_expression" => {
            let Some(function) = node.child_by_field_name("function") else {
                return;
            };
            let function = unwrap_generic_function(function);
            let arguments = node.child_by_field_name("arguments");
            calls.push(CallAst {
                receiver: receiver_for(function, source),
                callee: text(function, source),
                argument_names: arguments
                    .map(|arguments| argument_names(arguments, source))
                    .unwrap_or_default(),
                args_count: arguments.map(count_arguments).unwrap_or_default(),
                context,
                source_span: span(node),
            });
        }
        "macro_invocation" => {
            let Some(name) = node.child_by_field_name("macro") else {
                return;
            };
            calls.push(CallAst {
                receiver: None,
                callee: format!("{}!", text(name, source)),
                argument_names: Vec::new(),
                args_count: 0,
                context,
                source_span: span(node),
            });
        }
        _ => {}
    }
}

/// `foo::<T>()` calls `foo`; the turbofish is not part of the callee.
fn unwrap_generic_function(node: Node) -> Node {
    if node.kind() == "generic_function" {
        node.child_by_field_name("function").unwrap_or(node)
    } else {
        node
    }
}

impl SemanticFacts {
    fn extend_symbol(&mut self, symbol: &SymbolAst) {
        self.statements.extend(symbol.statements.clone());
        self.expressions.extend(symbol.expressions.clone());
        self.conditions.extend(symbol.conditions.clone());
        self.definitions.extend(symbol.definitions.clone());
        self.uses.extend(symbol.uses.clone());
        self.returns.extend(symbol.returns.clone());
        self.raises.extend(symbol.raises.clone());
        self.field_accesses.extend(symbol.field_accesses.clone());
        self.index_accesses.extend(symbol.index_accesses.clone());
    }
}

fn semantic_facts_in_scope(node: Node, source: &str, owner_id: &str) -> SemanticFacts {
    let mut facts = SemanticFacts::default();
    visit_semantic_descendants(node, &mut |descendant| {
        if let Some(statement) = statement_fact(descendant, source, owner_id) {
            facts.statements.push(statement);
        }
        if let Some(condition) = condition_fact(descendant, source, owner_id) {
            facts.conditions.push(condition);
        }
        if let Some(return_fact) = return_fact(descendant, source, owner_id) {
            facts.returns.push(return_fact);
        }
        if is_panic_macro(descendant, source) {
            facts.raises.push(RaiseAst {
                text: text(descendant, source),
                value: None,
                owner_id: owner_id.to_string(),
                source_span: span(descendant),
            });
        }
        facts
            .definitions
            .extend(definitions_for_node(descendant, source, owner_id));
        if let Some(expression) = expression_fact(descendant, source, owner_id) {
            facts.expressions.push(expression);
        }
        if let Some(use_fact) = use_fact(descendant, source, owner_id) {
            facts.uses.push(use_fact);
        }
        if descendant.kind() == "field_expression" {
            facts
                .field_accesses
                .push(field_access_fact(descendant, source, owner_id));
        }
        if descendant.kind() == "index_expression" {
            facts
                .index_accesses
                .push(index_access_fact(descendant, source, owner_id));
        }
    });
    facts
}

fn statement_fact(node: Node, source: &str, owner_id: &str) -> Option<StatementAst> {
    let kind = match node.kind() {
        "let_declaration" | "type_item" | "const_item" | "static_item" => {
            StatementKind::Declaration
        }
        "expression_statement" => {
            let inner = any_first_named_child(node)?;
            match inner.kind() {
                "assignment_expression" | "compound_assignment_expr" => StatementKind::Assignment,
                // Control-flow expressions and panics report themselves;
                // wrapping them in an expression statement would double
                // count them.
                _ if is_control_flow_expression(inner) || is_panic_macro(inner, source) => {
                    return None;
                }
                _ => StatementKind::Expression,
            }
        }
        "return_expression" => StatementKind::Return,
        "if_expression" => StatementKind::If,
        "for_expression" | "while_expression" | "loop_expression" => StatementKind::Loop,
        "break_expression" => StatementKind::Break,
        "continue_expression" => StatementKind::Continue,
        "match_expression" => StatementKind::Unknown,
        "function_item" => StatementKind::Function,
        "struct_item" | "enum_item" | "union_item" | "trait_item" | "impl_item" => {
            StatementKind::Class
        }
        "macro_invocation" if is_panic_macro(node, source) => StatementKind::Raise,
        _ => return None,
    };
    Some(StatementAst {
        kind,
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    })
}

fn is_control_flow_expression(node: Node) -> bool {
    matches!(
        node.kind(),
        "if_expression"
            | "for_expression"
            | "while_expression"
            | "loop_expression"
            | "match_expression"
            | "return_expression"
            | "break_expression"
            | "continue_expression"
    )
}

/// `panic!`-family macros abort the current path, which is the closest Rust
/// analogue of raising.
fn is_panic_macro(node: Node, source: &str) -> bool {
    if node.kind() != "macro_invocation" {
        return false;
    }
    let Some(name) = node.child_by_field_name("macro") else {
        return false;
    };
    let name = text(name, source);
    let name = name.rsplit("::").next().unwrap_or(&name);
    matches!(
        name,
        "panic" | "unreachable" | "todo" | "unimplemented"
    )
}

fn condition_fact(node: Node, source: &str, owner_id: &str) -> Option<ConditionAst> {
    let (kind, condition) = match node.kind() {
        "if_expression" => (ConditionKind::If, node.child_by_field_name("condition")?),
        "while_expression" => (ConditionKind::While, node.child_by_field_name("condition")?),
        "for_expression" => (ConditionKind::For, node.child_by_field_name("value")?),
        _ => return None,
    };
    Some(ConditionAst {
        kind,
        text: text(condition, source),
        owner_id: owner_id.to_string(),
        source_span: span(condition),
    })
}

fn return_fact(node: Node, source: &str, owner_id: &str) -> Option<ReturnAst> {
    if node.kind() != "return_expression" {
        return None;
    }
    Some(ReturnAst {
        value: any_first_named_child(node).map(|value| text(value, source)),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    })
}

fn definitions_for_node(node: Node, source: &str, owner_id: &str) -> Vec<DefinitionAst> {
    match node.kind() {
        "let_declaration" | "for_expression" | "match_arm" | "let_condition" => node
            .child_by_field_name("pattern")
            .map(|pattern| pattern_definitions(pattern, source, owner_id))
            .unwrap_or_default(),
        "const_item" | "static_item" => named_definition(node, DefinitionKind::Variable, source, owner_id),
        "type_item" => named_definition(node, DefinitionKind::Type, source, owner_id),
        "assignment_expression" | "compound_assignment_expr" => {
            let Some(left) = node.child_by_field_name("left") else {
                return Vec::new();
            };
            vec![DefinitionAst {
                name: definition_name(left, source),
                kind: definition_kind(left),
                text: text(left, source),
                owner_id: owner_id.to_string(),
                source_span: span(left),
            }]
        }
        _ => Vec::new(),
    }
}

fn named_definition(
    node: Node,
    kind: DefinitionKind,
    source: &str,
    owner_id: &str,
) -> Vec<DefinitionAst> {
    node.child_by_field_name("name")
        .map(|name| {
            vec![DefinitionAst {
                name: text(name, source),
                kind,
                text: text(name, source),
                owner_id: owner_id.to_string(),
                source_span: span(name),
            }]
        })
        .unwrap_or_default()
}

fn pattern_definitions(pattern: Node, source: &str, owner_id: &str) -> Vec<DefinitionAst> {
    let mut identifiers = Vec::new();
    collect_pattern_identifiers(pattern, source, &mut identifiers);
    identifiers
        .into_iter()
        .map(|identifier| DefinitionAst {
            name: text(identifier, source),
            kind: DefinitionKind::Variable,
            text: text(identifier, source),
            owner_id: owner_id.to_string(),
            source_span: span(identifier),
        })
        .collect()
}

/// Collect the variables a pattern binds. Capitalized identifiers are enum
/// variants or constants rather than bindings, and the path of a
/// tuple-struct or struct pattern names a constructor.
fn collect_pattern_identifiers<'tree>(
    node: Node<'tree>,
    source: &str,
    out: &mut Vec<Node<'tree>>,
) {
    match node.kind() {
        "identifier" => {
            if !text(node, source)
                .chars()
                .next()
                .is_some_and(char::is_uppercase)
            {
                out.push(node);
            }
        }
        "shorthand_field_identifier" => out.push(node),
        "scoped_identifier" | "type_identifier" | "field_identifier" | "scoped_type_identifier" => {}
        "tuple_struct_pattern" | "struct_pattern" => {
            let type_id = node.child_by_field_name("type").map(|node| node.id());
            for child in named_children(node) {
                if Some(child.id()) != type_id {
                    collect_pattern_identifiers(child, source, out);
                }
            }
        }
        "field_pattern" => {
            if let Some(pattern) = node
                .child_by_field_name("pattern")
                .or_else(|| node.child_by_field_name("name"))
            {
                collect_pattern_identifiers(pattern, source, out);
            }
        }
        _ => {
            for child in named_children(node) {
                collect_pattern_identifiers(child, source, out);
            }
        }
    }
}

fn definitions_for_import(import: &ImportAst, owner_id: &str) -> Vec<DefinitionAst> {
    import
        .names
        .iter()
        .map(|name| DefinitionAst {
            name: name.alias.clone().unwrap_or_else(|| name.name.clone()),
            kind: DefinitionKind::Import,
            text: name.name.clone(),
            owner_id: owner_id.to_string(),
            source_span: import.source_span,
        })
        .collect()
}

fn definition_for_param(parameter: &ParamAst, owner_id: &str) -> DefinitionAst {
    DefinitionAst {
        name: parameter.name.clone(),
        kind: DefinitionKind::Parameter,
        text: parameter.text.clone(),
        owner_id: owner_id.to_string(),
        source_span: parameter.source_span,
    }
}

fn expression_fact(node: Node, source: &str, owner_id: &str) -> Option<ExpressionAst> {
    let kind = match node.kind() {
        "identifier" | "field_identifier" => ExpressionKind::Identifier,
        "string_literal" | "raw_string_literal" | "char_literal" | "integer_literal"
        | "float_literal" | "boolean_literal" => ExpressionKind::Literal,
        "call_expression" | "macro_invocation" => ExpressionKind::Call,
        "field_expression" => ExpressionKind::FieldAccess,
        "index_expression" => ExpressionKind::IndexAccess,
        "assignment_expression" | "compound_assignment_expr" => ExpressionKind::Assignment,
        "binary_expression" => ExpressionKind::BinaryOperator,
        "unary_expression" => ExpressionKind::UnaryOperator,
        "if_expression" if is_value_position(node) => ExpressionKind::Conditional,
        "await_expression" => ExpressionKind::Await,
        "yield_expression" => ExpressionKind::Yield,
        _ => return None,
    };
    Some(ExpressionAst {
        kind,
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    })
}

/// An `if` used for its value (`let x = if c { a } else { b };`) is Rust's
/// conditional expression; an `if` in statement position is not.
fn is_value_position(node: Node) -> bool {
    !node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "expression_statement" | "block" | "else_clause"
        )
    })
}

fn use_fact(node: Node, source: &str, owner_id: &str) -> Option<UseAst> {
    let kind = match node.kind() {
        "identifier" => UseKind::Identifier,
        "field_identifier" => UseKind::Field,
        _ => return None,
    };
    // Segments of a path (`std::mem::swap`) are not uses of local names.
    if node
        .parent()
        .is_some_and(|parent| parent.kind() == "scoped_identifier")
    {
        return None;
    }
    Some(UseAst {
        name: text(node, source),
        kind,
        owner_id: owner_id.to_string(),
        source_span: span(node),
    })
}

fn field_access_fact(node: Node, source: &str, owner_id: &str) -> FieldAccessAst {
    FieldAccessAst {
        object: node
            .child_by_field_name("value")
            .map(|object| text(object, source)),
        field: node
            .child_by_field_name("field")
            .map(|field| text(field, source))
            .unwrap_or_else(|| text(node, source)),
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    }
}

fn index_access_fact(node: Node, source: &str, owner_id: &str) -> IndexAccessAst {
    let children = named_children(node);
    IndexAccessAst {
        object: children.first().map(|object| text(*object, source)),
        index: children.get(1).map(|index| text(*index, source)),
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    }
}

fn definition_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => text(node, source),
        "field_expression" => node
            .child_by_field_name("field")
            .map(|field| text(field, source))
            .unwrap_or_else(|| text(node, source)),
        _ => binding_name(node, source),
    }
}

fn definition_kind(node: Node) -> DefinitionKind {
    match node.kind() {
        "identifier" => DefinitionKind::Variable,
        "field_expression" => DefinitionKind::Field,
        _ => DefinitionKind::Unknown,
    }
}

fn module_owner_id(module_path: &str) -> String {
    format!("{module_path}:<module>")
}

fn qualified_symbol_name(symbol: &SymbolAst) -> String {
    symbol
        .parent
        .as_ref()
        .map(|parent| format!("{parent}.{}", symbol.name))
        .unwrap_or_else(|| symbol.name.clone())
}

fn visit_semantic_descendants(node: Node, visitor: &mut impl FnMut(Node)) {
    for child in named_children(node) {
        visitor(child);
        if is_nested_definition(child) {
            continue;
        }
        visit_semantic_descendants(child, visitor);
    }
}

fn is_nested_definition(node: Node) -> bool {
    matches!(
        node.kind(),
        "function_item"
            | "impl_item"
            | "trait_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "mod_item"
            | "closure_expression"
    )
}

fn visit_top_level_call_descendants(node: Node, visitor: &mut impl FnMut(Node)) {
    visitor(node);
    for child in named_children(node) {
        if is_nested_definition(child) {
            continue;
        }
        visit_top_level_call_descendants(child, visitor);
    }
}

fn receiver_for(node: Node, source: &str) -> Option<String> {
    if node.kind() != "field_expression" {
        return None;
    }
    node.child_by_field_name("value")
        .map(|value| text(value, source))
}

fn count_arguments(node: Node) -> usize {
    named_children(node)
        .into_iter()
        .filter(|child| !is_comment(*child))
        .count()
}

fn argument_names(node: Node, source: &str) -> Vec<String> {
    named_children(node)
        .into_iter()
        .filter_map(|child| match child.kind() {
            "identifier" => Some(text(child, source)),
            _ => None,
        })
        .collect()
}

fn is_comment(node: Node) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

/// Dotted module path for a source file, following Rust's file-to-module
/// layout: `lib.rs`, `main.rs` and `mod.rs` name their containing directory.
pub(crate) fn module_path(relative_path: &str) -> String {
    let without_extension = relative_path.trim_end_matches(".rs");
    let module = ["/mod", "/lib", "/main"]
        .iter()
        .find_map(|suffix| without_extension.strip_suffix(suffix))
        .or_else(|| matches!(without_extension, "mod" | "lib" | "main").then_some(""))
        .unwrap_or(without_extension);
    module.replace('/', ".")
}

/// The bare type name of an `impl` target, e.g. `Foo` for `Foo<T>` or `&Foo`.
fn type_name(node: Node, source: &str) -> String {
    let inner = match node.kind() {
        "generic_type" | "reference_type" => node.child_by_field_name("type"),
        "scoped_type_identifier" => node.child_by_field_name("name"),
        _ => None,
    };
    match inner {
        Some(inner) => type_name(inner, source),
        None => text(node, source),
    }
}

fn binding_name(node: Node, source: &str) -> String {
    if node.kind() == "identifier" {
        return text(node, source);
    }
    let mut identifiers = Vec::new();
    collect_pattern_identifiers(node, source, &mut identifiers);
    identifiers
        .first()
        .map(|identifier| text(*identifier, source))
        .unwrap_or_else(|| text(node, source))
}

fn any_first_named_child(node: Node) -> Option<Node> {
    named_children(node)
        .into_iter()
        .find(|child| !is_comment(*child))
}

/// Named children of `node`. Children inside an `ERROR` node are lifted into the result so
/// the well-formed constructs around a syntax error are still extracted.
fn named_children(node: Node) -> Vec<Node> {
    let mut children = Vec::new();
    push_named_children(node, &mut children);
    children
}

fn push_named_children<'tree>(node: Node<'tree>, children: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.is_error() {
            push_named_children(child, children);
        } else {
            children.push(child);
        }
    }
}

fn visit_named_descendants(node: Node, visitor: &mut impl FnMut(Node)) {
    for child in named_children(node) {
        visitor(child);
        visit_named_descendants(child, visitor);
    }
}

fn text(node: Node, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn span(node: Node) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    SourceSpan {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: start.row,
        start_column: start.column,
        end_row: end.row,
        end_column: end.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(path: &str, source: &str) -> FileAst {
        let parsed = parse_rust_tree(path.to_string(), source.to_string())
            .expect("parse Rust fixture");
        filter_file(&parsed)
    }

    #[test]
    fn parser_emits_required_statement_kinds() {
        let file = parse(
            "sample.rs",
            r#"
struct Example;

impl Example {
    fn method(&self) {}
}

fn run(value: i32) -> i32 {
    let mut result = value;
    result = value + 1;
    effect(result);
    if result > 0 {
        return result;
    }
    while result > 0 {
        panic!("boom");
    }
    loop {
        break;
    }
    match result {
        _ => {}
    }
    result
}
"#,
        );
        let kinds = file
            .statements
            .iter()
            .map(|statement| statement.kind)
            .collect::<Vec<_>>();

        for kind in [
            StatementKind::Declaration,
            StatementKind::Assignment,
            StatementKind::Expression,
            StatementKind::If,
            StatementKind::Loop,
            StatementKind::Return,
            StatementKind::Raise,
            StatementKind::Break,
            StatementKind::Function,
            StatementKind::Class,
            StatementKind::Unknown,
        ] {
            assert!(kinds.contains(&kind), "missing Rust statement kind {kind:?}");
        }
        let run = file
            .symbols
            .iter()
            .find(|symbol| symbol.name == "run")
            .expect("run symbol");
        assert_eq!(
            run.returns.len(),
            2,
            "explicit return plus implicit tail return"
        );
        assert!(
            file.statements
                .iter()
                .all(|statement| statement.text != "panic!(\"boom\");"),
            "panic statement should not be double counted as an expression"
        );
    }

    #[test]
    fn parser_emits_required_expression_kinds() {
        let file = parse(
            "sample.rs",
            r#"
async fn run(obj: Thing, items: Vec<i32>, index: usize, value: i32) -> i32 {
    let mut result = value + 1;
    send(value, obj.field, items[index], 42);
    let choice = if result > 0 { value } else { 0 };
    result = -value;
    send(value).await;
    result
}
"#,
        );
        let kinds = file
            .expressions
            .iter()
            .map(|expression| expression.kind)
            .collect::<Vec<_>>();

        for kind in [
            ExpressionKind::Identifier,
            ExpressionKind::Literal,
            ExpressionKind::Call,
            ExpressionKind::FieldAccess,
            ExpressionKind::IndexAccess,
            ExpressionKind::Assignment,
            ExpressionKind::BinaryOperator,
            ExpressionKind::UnaryOperator,
            ExpressionKind::Conditional,
            ExpressionKind::Await,
        ] {
            assert!(kinds.contains(&kind), "missing Rust expression kind {kind:?}");
        }
    }

    #[test]
    fn parser_attaches_impl_and_trait_methods_to_their_types() {
        let file = parse(
            "shapes/mod.rs",
            r#"
pub struct Circle<T> { radius: T }

trait Area {
    fn area(&self) -> f64;
    fn describe(&self) -> String { String::new() }
}

impl<T> Circle<T> {
    pub fn new(radius: T) -> Self { Circle { radius } }
}

impl Area for Circle<f64> {
    fn area(&self) -> f64 { self.radius }
}
"#,
        );
        let ids = file
            .symbols
            .iter()
            .map(|symbol| (symbol.id.as_str(), symbol.kind))
            .collect::<Vec<_>>();

        assert!(ids.contains(&("shapes:Circle", SymbolKind::Class)));
        assert!(ids.contains(&("shapes:Area", SymbolKind::Class)));
        assert!(ids.contains(&("shapes:Area.describe", SymbolKind::Method)));
        assert!(ids.contains(&("shapes:Circle.new", SymbolKind::Method)));
        assert!(ids.contains(&("shapes:Circle.area", SymbolKind::Method)));
        assert!(
            !ids.contains(&("shapes:Area.area", SymbolKind::Method)),
            "bodiless trait signatures are not callables"
        );
    }

    #[test]
    fn parser_flattens_use_trees() {
        let file = parse(
            "sample.rs",
            r#"
use std::collections::{HashMap, hash_map::Entry as MapEntry};
use crate::shapes::{self, Circle};
use serde;
use std::io::*;
"#,
        );
        let imports = file
            .imports
            .iter()
            .map(|import| {
                (
                    import.module.clone(),
                    import
                        .names
                        .iter()
                        .map(|name| (name.name.clone(), name.alias.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();

        assert!(imports.contains(&(
            Some("std.collections".to_string()),
            vec![("HashMap".to_string(), None)]
        )));
        assert!(imports.contains(&(
            Some("std.collections.hash_map".to_string()),
            vec![("Entry".to_string(), Some("MapEntry".to_string()))]
        )));
        assert!(imports.contains(&(
            Some("crate".to_string()),
            vec![("shapes".to_string(), None)]
        )));
        assert!(imports.contains(&(
            Some("crate.shapes".to_string()),
            vec![("Circle".to_string(), None)]
        )));
        assert!(imports.contains(&(None, vec![("serde".to_string(), None)])));
        assert!(imports.contains(&(Some("std.io".to_string()), Vec::new())));
    }

    #[test]
    fn parser_emits_module_initializer_calls() {
        let file = parse(
            "sample.rs",
            r#"
use pkg::external_boot;

static CREATED: Lazy<Client> = make_client();

fn run() {
    body_only();
}
"#,
        );
        let callees = file
            .calls
            .iter()
            .map(|call| call.callee.as_str())
            .collect::<Vec<_>>();

        assert!(callees.contains(&"make_client"));
        assert!(!callees.contains(&"body_only"));
        assert!(
            file.calls
                .iter()
                .all(|call| call.context == CallContext::ModuleInitializer)
        );
    }

    #[test]
    fn module_paths_follow_rust_file_layout() {
        assert_eq!(module_path("src/lib.rs"), "src");
        assert_eq!(module_path("src/main.rs"), "src");
        assert_eq!(module_path("src/shapes/mod.rs"), "src.shapes");
        assert_eq!(module_path("src/shapes/circle.rs"), "src.shapes.circle");
        assert_eq!(module_path("lib.rs"), "");
    }
}
