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

pub struct ParsedTypeScriptFile {
    pub relative_path: String,
    source: String,
    tree: Tree,
}

pub fn parse_typescript_file(path: &Path, relative_path: String) -> Result<ParsedTypeScriptFile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read TypeScript source {}", path.display()))?;
    parse_typescript_tree(relative_path, source)
}

fn parse_typescript_tree(relative_path: String, source: String) -> Result<ParsedTypeScriptFile> {
    let mut parser = Parser::new();
    let language = if relative_path.ends_with(".tsx") {
        tree_sitter_typescript::LANGUAGE_TSX
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    };
    let language = language.into();
    parser
        .set_language(&language)
        .context("failed to configure Tree-sitter TypeScript parser")?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow!("Tree-sitter failed to parse {}", relative_path))?;
    Ok(ParsedTypeScriptFile {
        relative_path,
        source,
        tree,
    })
}

pub fn filter_file(parsed: &ParsedTypeScriptFile) -> FileAst {
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
        "import_statement" => imports.push(import_ast(node, source)),
        "export_statement" => {
            for child in named_children(node) {
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
        "lexical_declaration" | "variable_declaration" => {
            assignments.extend(extract_variable_assignments(node, source));
            calls.extend(top_level_calls_in(node, source));
            for child in named_children(node) {
                if child.kind() == "variable_declarator" {
                    collect_variable_function(child, source, module_path, symbols);
                }
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            collect_function(node, source, module_path, None, symbols);
        }
        "class_declaration" => {
            collect_class(node, source, module_path, symbols);
        }
        "type_alias_declaration" | "interface_declaration" | "enum_declaration" => {
            if let Some(assignment) = extract_named_assignment(node, source) {
                assignments.push(assignment);
            }
        }
        _ => calls.extend(top_level_calls_in(node, source)),
    }
}

fn collect_class(node: Node, source: &str, module_path: &str, symbols: &mut Vec<SymbolAst>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = binding_name(name_node, source);
    let id = format!("{module_path}:{name}");
    let facts = node
        .child_by_field_name("body")
        .map(|body| semantic_facts_in_scope(body, source, &id))
        .unwrap_or_default();

    symbols.push(SymbolAst {
        id: id.clone(),
        name: name.clone(),
        kind: SymbolKind::Class,
        module_path: module_path.to_string(),
        parent: None,
        parameters: Vec::new(),
        decorators: Vec::new(),
        return_type: None,
        body_span: node.child_by_field_name("body").map(span),
        assignments: node
            .child_by_field_name("body")
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

    visit_named_descendants(node, &mut |descendant| {
        if descendant.kind() == "method_definition" {
            collect_function(descendant, source, module_path, Some(&name), symbols);
        }
    });
}

fn collect_variable_function(
    node: Node,
    source: &str,
    module_path: &str,
    symbols: &mut Vec<SymbolAst>,
) {
    let Some(value) = node.child_by_field_name("value") else {
        return;
    };
    if !matches!(value.kind(), "arrow_function" | "function_expression") {
        return;
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };

    let name = binding_name(name_node, source);
    let id = format!("{module_path}:{name}");
    let parameters = value
        .child_by_field_name("parameters")
        .map(|parameters| params_in(parameters, source))
        .unwrap_or_default();
    let mut facts = value
        .child_by_field_name("body")
        .map(|body| semantic_facts_in_scope(body, source, &id))
        .unwrap_or_default();
    facts.definitions.extend(
        parameters
            .iter()
            .map(|parameter| definition_for_param(parameter, &id)),
    );
    symbols.push(SymbolAst {
        id,
        name,
        kind: SymbolKind::Function,
        module_path: module_path.to_string(),
        parent: None,
        parameters,
        decorators: Vec::new(),
        return_type: value
            .child_by_field_name("return_type")
            .map(|return_type| text(return_type, source)),
        body_span: value.child_by_field_name("body").map(span),
        assignments: value
            .child_by_field_name("body")
            .map(|body| assignments_in(body, source))
            .unwrap_or_default(),
        calls: calls_in(value, source, CallContext::Body),
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
    let name = binding_name(name_node, source);
    let qualified_name = parent
        .map(|parent| format!("{parent}.{name}"))
        .unwrap_or_else(|| name.clone());
    let id = format!("{module_path}:{qualified_name}");
    let parameters = node
        .child_by_field_name("parameters")
        .map(|parameters| params_in(parameters, source))
        .unwrap_or_default();
    let mut facts = node
        .child_by_field_name("body")
        .map(|body| semantic_facts_in_scope(body, source, &id))
        .unwrap_or_default();
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
        body_span: node.child_by_field_name("body").map(span),
        assignments: node
            .child_by_field_name("body")
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

fn import_ast(node: Node, source: &str) -> ImportAst {
    let import_text = text(node, source);
    let (module, names) = parse_import(&import_text);

    ImportAst {
        text: import_text,
        module,
        names,
        source_span: span(node),
    }
}

fn parse_import(import_text: &str) -> (Option<String>, Vec<ImportNameAst>) {
    let rest = import_text
        .trim_end_matches(';')
        .trim_start_matches("import type ")
        .trim_start_matches("import ")
        .trim();

    if let Some(module) = rest
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    {
        return (Some(module.to_string()), Vec::new());
    }

    let Some((names, module)) = rest.split_once(" from ") else {
        return (None, Vec::new());
    };
    let module = Some(
        module
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string(),
    );
    let names = parse_import_names(names);

    (module, names)
}

fn parse_import_names(names: &str) -> Vec<ImportNameAst> {
    let names = names.trim();
    if let Some(named_imports) = names
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
    {
        return named_imports
            .split(',')
            .filter_map(|name| parse_import_name(name.trim()))
            .collect();
    }

    parse_import_name(names).into_iter().collect()
}

fn parse_import_name(name: &str) -> Option<ImportNameAst> {
    let name = name.trim().trim_start_matches("type ").trim();
    if name.is_empty() {
        return None;
    }

    let (name, alias) = name
        .split_once(" as ")
        .map(|(name, alias)| (name.trim(), Some(alias.trim().to_string())))
        .unwrap_or((name, None));

    Some(ImportNameAst {
        name: name.to_string(),
        alias,
    })
}

fn extract_variable_assignments(node: Node, source: &str) -> Vec<AssignmentAst> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "variable_declarator")
        .filter_map(|declarator| {
            let target = declarator
                .child_by_field_name("name")
                .map(|target| binding_name(target, source))?;

            Some(AssignmentAst {
                target,
                value: declarator
                    .child_by_field_name("value")
                    .map(|value| text(value, source)),
                text: text(declarator, source),
                source_span: span(declarator),
            })
        })
        .collect()
}

fn extract_named_assignment(node: Node, source: &str) -> Option<AssignmentAst> {
    let name = node.child_by_field_name("name")?;
    Some(AssignmentAst {
        target: text(name, source),
        value: None,
        text: text(node, source),
        source_span: span(node),
    })
}

fn assignments_in(node: Node, source: &str) -> Vec<AssignmentAst> {
    let mut assignments = Vec::new();
    visit_named_descendants(node, &mut |descendant| {
        if matches!(
            descendant.kind(),
            "lexical_declaration" | "variable_declaration"
        ) {
            assignments.extend(extract_variable_assignments(descendant, source));
        }
    });
    assignments
}

fn params_in(node: Node, source: &str) -> Vec<ParamAst> {
    named_children(node)
        .into_iter()
        .filter_map(|child| match child.kind() {
            "required_parameter" | "optional_parameter" => {
                let name_node = child
                    .child_by_field_name("pattern")
                    .or_else(|| child.child_by_field_name("name"))
                    .or_else(|| first_named_child(child, "identifier"))?;
                Some(ParamAst {
                    name: text(name_node, source),
                    text: text(child, source),
                    source_span: span(child),
                })
            }
            "identifier" => Some(ParamAst {
                name: text(child, source),
                text: text(child, source),
                source_span: span(child),
            }),
            _ => None,
        })
        .collect()
}

fn decorators_in(node: Node, source: &str) -> Vec<DecoratorAst> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "decorator")
        .map(|decorator| DecoratorAst {
            text: text(decorator, source)
                .trim_start_matches('@')
                .trim()
                .to_string(),
            source_span: span(decorator),
        })
        .collect()
}

fn calls_in(node: Node, source: &str, context: CallContext) -> Vec<CallAst> {
    let mut calls = Vec::new();
    visit_named_descendants(node, &mut |descendant| match descendant.kind() {
        "call_expression" | "new_expression" => {
            let Some(function) = descendant
                .child_by_field_name("function")
                .or_else(|| descendant.child_by_field_name("constructor"))
            else {
                return;
            };
            calls.push(CallAst {
                receiver: receiver_for(function, source),
                callee: text(function, source),
                argument_names: descendant
                    .child_by_field_name("arguments")
                    .map(|arguments| argument_names(arguments, source))
                    .unwrap_or_default(),
                args_count: descendant
                    .child_by_field_name("arguments")
                    .map(count_arguments)
                    .unwrap_or_default(),
                context,
                source_span: span(descendant),
            });
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            let Some(name) = descendant
                .child_by_field_name("name")
                .or_else(|| first_named_child(descendant, "identifier"))
            else {
                return;
            };
            let callee = text(name, source);
            if !callee
                .chars()
                .next()
                .map(char::is_uppercase)
                .unwrap_or(false)
            {
                return;
            }

            calls.push(CallAst {
                receiver: None,
                callee,
                argument_names: jsx_attribute_names(descendant, source),
                args_count: jsx_attribute_count(descendant),
                context,
                source_span: span(descendant),
            });
        }
        _ => {}
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
        "call_expression" | "new_expression" => {
            let Some(function) = node
                .child_by_field_name("function")
                .or_else(|| node.child_by_field_name("constructor"))
            else {
                return;
            };
            calls.push(CallAst {
                receiver: receiver_for(function, source),
                callee: text(function, source),
                argument_names: node
                    .child_by_field_name("arguments")
                    .map(|arguments| argument_names(arguments, source))
                    .unwrap_or_default(),
                args_count: node
                    .child_by_field_name("arguments")
                    .map(count_arguments)
                    .unwrap_or_default(),
                context,
                source_span: span(node),
            });
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            let Some(name) = node
                .child_by_field_name("name")
                .or_else(|| first_named_child(node, "identifier"))
            else {
                return;
            };
            let callee = text(name, source);
            if !callee
                .chars()
                .next()
                .map(char::is_uppercase)
                .unwrap_or(false)
            {
                return;
            }

            calls.push(CallAst {
                receiver: None,
                callee,
                argument_names: jsx_attribute_names(node, source),
                args_count: jsx_attribute_count(node),
                context,
                source_span: span(node),
            });
        }
        _ => {}
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
        if descendant.kind() == "throw_statement" {
            facts.raises.push(RaiseAst {
                text: text(descendant, source),
                value: any_first_named_child(descendant).map(|value| text(value, source)),
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
        if descendant.kind() == "member_expression" {
            facts
                .field_accesses
                .push(field_access_fact(descendant, source, owner_id));
        }
        if descendant.kind() == "subscript_expression" {
            facts
                .index_accesses
                .push(index_access_fact(descendant, source, owner_id));
        }
    });
    facts
}

fn statement_fact(node: Node, source: &str, owner_id: &str) -> Option<StatementAst> {
    let kind = match node.kind() {
        "interface_declaration"
        | "lexical_declaration"
        | "type_alias_declaration"
        | "variable_declaration" => StatementKind::Declaration,
        "expression_statement" if expression_statement_is_assignment(node) => {
            StatementKind::Assignment
        }
        "expression_statement" => StatementKind::Expression,
        "return_statement" => StatementKind::Return,
        "throw_statement" => StatementKind::Throw,
        "if_statement" => StatementKind::If,
        "for_statement" | "for_in_statement" | "while_statement" | "do_statement" => {
            StatementKind::Loop
        }
        "function_declaration" | "generator_function_declaration" => StatementKind::Function,
        "method_definition" => StatementKind::Function,
        "class_declaration" => StatementKind::Class,
        "break_statement" => StatementKind::Break,
        "continue_statement" => StatementKind::Continue,
        "try_statement" => StatementKind::Try,
        "catch_clause" => StatementKind::Catch,
        "finally_clause" => StatementKind::Finally,
        "debugger_statement" | "empty_statement" | "switch_statement" => StatementKind::Unknown,
        _ => return None,
    };
    Some(StatementAst {
        kind,
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    })
}

fn condition_fact(node: Node, source: &str, owner_id: &str) -> Option<ConditionAst> {
    let (kind, condition) = match node.kind() {
        "if_statement" => (ConditionKind::If, node.child_by_field_name("condition")?),
        "while_statement" | "do_statement" => {
            (ConditionKind::While, node.child_by_field_name("condition")?)
        }
        "for_statement" => (ConditionKind::For, node.child_by_field_name("condition")?),
        "ternary_expression" => (
            ConditionKind::ConditionalExpression,
            node.child_by_field_name("condition")?,
        ),
        _ => return None,
    };
    let condition = unparenthesized(condition);
    Some(ConditionAst {
        kind,
        text: text(condition, source),
        owner_id: owner_id.to_string(),
        source_span: span(condition),
    })
}

fn unparenthesized(node: Node) -> Node {
    if node.kind() == "parenthesized_expression" {
        any_first_named_child(node).unwrap_or(node)
    } else {
        node
    }
}

fn return_fact(node: Node, source: &str, owner_id: &str) -> Option<ReturnAst> {
    if node.kind() != "return_statement" {
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
        "variable_declarator" => {
            let Some(name) = node.child_by_field_name("name") else {
                return Vec::new();
            };
            vec![DefinitionAst {
                name: binding_name(name, source),
                kind: DefinitionKind::Variable,
                text: text(name, source),
                owner_id: owner_id.to_string(),
                source_span: span(name),
            }]
        }
        "assignment_expression" => {
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
        "type_alias_declaration" | "interface_declaration" | "enum_declaration" => {
            let Some(name) = node.child_by_field_name("name") else {
                return Vec::new();
            };
            vec![DefinitionAst {
                name: text(name, source),
                kind: DefinitionKind::Type,
                text: text(name, source),
                owner_id: owner_id.to_string(),
                source_span: span(name),
            }]
        }
        _ => Vec::new(),
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
        "identifier" | "property_identifier" => ExpressionKind::Identifier,
        "string" | "template_string" | "number" | "true" | "false" | "null" | "undefined" => {
            ExpressionKind::Literal
        }
        "call_expression" | "new_expression" => ExpressionKind::Call,
        "member_expression" => ExpressionKind::FieldAccess,
        "subscript_expression" => ExpressionKind::IndexAccess,
        "assignment_expression" => ExpressionKind::Assignment,
        "binary_expression" => ExpressionKind::BinaryOperator,
        "unary_expression" => ExpressionKind::UnaryOperator,
        "ternary_expression" => ExpressionKind::Conditional,
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

fn use_fact(node: Node, source: &str, owner_id: &str) -> Option<UseAst> {
    let kind = match node.kind() {
        "identifier" => UseKind::Identifier,
        "property_identifier" => UseKind::Field,
        _ => return None,
    };
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
            .child_by_field_name("object")
            .map(|object| text(object, source)),
        field: node
            .child_by_field_name("property")
            .map(|property| text(property, source))
            .unwrap_or_else(|| text(node, source)),
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    }
}

fn index_access_fact(node: Node, source: &str, owner_id: &str) -> IndexAccessAst {
    IndexAccessAst {
        object: node
            .child_by_field_name("object")
            .or_else(|| any_first_named_child(node))
            .map(|object| text(object, source)),
        index: node
            .child_by_field_name("index")
            .map(|index| text(index, source)),
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    }
}

fn definition_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => text(node, source),
        "member_expression" => node
            .child_by_field_name("property")
            .map(|property| text(property, source))
            .unwrap_or_else(|| text(node, source)),
        _ => binding_name(node, source),
    }
}

fn definition_kind(node: Node) -> DefinitionKind {
    match node.kind() {
        "identifier" => DefinitionKind::Variable,
        "member_expression" => DefinitionKind::Field,
        _ => DefinitionKind::Unknown,
    }
}

fn expression_statement_is_assignment(node: Node) -> bool {
    named_children(node)
        .into_iter()
        .any(|child| child.kind() == "assignment_expression")
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
        "class_declaration"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "arrow_function"
            | "function_expression"
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
    if node.kind() != "member_expression" {
        return None;
    }
    node.child_by_field_name("object")
        .map(|object| text(object, source))
}

fn count_arguments(node: Node) -> usize {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind() != "comment")
        .count()
}

fn jsx_attribute_count(node: Node) -> usize {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "jsx_attribute" || child.kind() == "jsx_expression")
        .count()
}

fn argument_names(node: Node, source: &str) -> Vec<String> {
    named_children(node)
        .into_iter()
        .filter_map(|child| match child.kind() {
            "identifier" | "property_identifier" => Some(text(child, source)),
            "assignment_expression" => child
                .child_by_field_name("left")
                .map(|left| text(left, source)),
            _ => None,
        })
        .collect()
}

fn jsx_attribute_names(node: Node, source: &str) -> Vec<String> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "jsx_attribute")
        .filter_map(|attribute| {
            attribute
                .child_by_field_name("name")
                .or_else(|| first_named_child(attribute, "property_identifier"))
                .or_else(|| first_named_child(attribute, "identifier"))
                .map(|name| text(name, source))
        })
        .collect()
}

fn module_path(relative_path: &str) -> String {
    relative_path
        .trim_end_matches(".tsx")
        .trim_end_matches(".ts")
        .replace('/', ".")
}

fn binding_name(node: Node, source: &str) -> String {
    if node.kind() == "identifier" {
        text(node, source)
    } else {
        first_named_child(node, "identifier")
            .map(|identifier| text(identifier, source))
            .unwrap_or_else(|| text(node, source))
    }
}

fn first_named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind() == kind)
}

fn any_first_named_child(node: Node) -> Option<Node> {
    named_children(node).into_iter().next()
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
        start_byte: node.start_byte() as u32,
        end_byte: node.end_byte() as u32,
        start_row: start.row as u32,
        start_column: start.column as u32,
        end_row: end.row as u32,
        end_column: end.column as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sg020_typescript_parser_emits_required_statement_kinds() {
        let parsed = parse_typescript_tree(
            "sample.ts".to_string(),
            r#"
class Example {
  method() {
    debugger;
  }
}

function run(value: number) {
  let result = value;
  result = value + 1;
  effect(result);
  if (result) {
    return result;
  }
  while (result) {
    throw new Error();
  }
  switch (result) {
    default:
      break;
  }
}
"#
            .to_string(),
        )
        .expect("parse TypeScript fixture");
        let file = filter_file(&parsed);
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
            StatementKind::Throw,
            StatementKind::Function,
            StatementKind::Class,
            StatementKind::Unknown,
        ] {
            assert!(
                kinds.contains(&kind),
                "missing TypeScript statement kind {kind:?}"
            );
        }
    }

    #[test]
    fn sg021_typescript_parser_emits_required_expression_kinds() {
        let parsed = parse_typescript_tree(
            "sample.ts".to_string(),
            r#"
async function* run(obj: any, items: number[], index: number, value: number) {
  let result = value + 1;
  send(value, obj.field, items[index], 42);
  const choice = result ? value : 0;
  result = -value;
  await send(value);
  yield result;
}
"#
            .to_string(),
        )
        .expect("parse TypeScript expression fixture");
        let file = filter_file(&parsed);
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
            ExpressionKind::Yield,
        ] {
            assert!(
                kinds.contains(&kind),
                "missing TypeScript expression kind {kind:?}"
            );
        }
    }

    #[test]
    fn sg042_typescript_parser_emits_module_initializer_calls() {
        let parsed = parse_typescript_tree(
            "sample.tsx".to_string(),
            r#"
import { externalBoot } from "pkg";

const created = makeClient();
externalBoot(created);
missingBoot();

function run() {
  bodyOnly();
}

const handler = () => {
  nestedOnly();
};

<Widget value={created} />;
"#
            .to_string(),
        )
        .expect("parse TypeScript module initializer fixture");
        let file = filter_file(&parsed);
        let callees = file
            .calls
            .iter()
            .map(|call| call.callee.as_str())
            .collect::<Vec<_>>();

        for callee in ["makeClient", "externalBoot", "missingBoot", "Widget"] {
            assert!(
                callees.contains(&callee),
                "missing TypeScript module-initializer call {callee}"
            );
        }
        assert!(
            !callees.contains(&"bodyOnly"),
            "function body calls should remain owned by the function callable"
        );
        assert!(
            !callees.contains(&"nestedOnly"),
            "arrow function body calls should not be module-initializer calls"
        );
        assert!(
            file.calls
                .iter()
                .all(|call| call.context == CallContext::ModuleInitializer),
            "top-level TypeScript calls should use ModuleInitializer context"
        );
    }
}
