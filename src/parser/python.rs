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

pub struct ParsedPythonFile {
    pub relative_path: String,
    source: String,
    tree: Tree,
}

pub fn parse_python_file(path: &Path, relative_path: String) -> Result<ParsedPythonFile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read Python source {}", path.display()))?;
    parse_python_tree(relative_path, source)
}

fn parse_python_tree(relative_path: String, source: String) -> Result<ParsedPythonFile> {
    let mut parser = Parser::new();
    let language = tree_sitter_python::LANGUAGE.into();
    parser
        .set_language(&language)
        .context("failed to configure Tree-sitter Python parser")?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow!("Tree-sitter failed to parse {}", relative_path))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Tree-sitter parse errors in {}", relative_path));
    }

    Ok(ParsedPythonFile {
        relative_path,
        source,
        tree,
    })
}

pub fn filter_file(parsed: &ParsedPythonFile) -> FileAst {
    let root = parsed.tree.root_node();
    let module_path = module_path(&parsed.relative_path);
    let mut imports = Vec::new();
    let mut assignments = Vec::new();
    let mut calls = Vec::new();
    let mut symbols = Vec::new();

    for child in named_children(root) {
        match child.kind() {
            "import_statement" | "import_from_statement" => {
                imports.push(import_ast(child, &parsed.source));
            }
            "expression_statement" => {
                assignments.extend(extract_assignment(child, &parsed.source));
                calls.extend(calls_in(
                    child,
                    &parsed.source,
                    CallContext::ModuleInitializer,
                ));
            }
            "class_definition" => {
                collect_class(
                    child,
                    &parsed.source,
                    &module_path,
                    Vec::new(),
                    &mut symbols,
                );
            }
            "function_definition" => {
                collect_function(
                    child,
                    &parsed.source,
                    &module_path,
                    None,
                    Vec::new(),
                    &mut symbols,
                );
            }
            "decorated_definition" => {
                calls.extend(decorator_calls(child, &parsed.source));
                collect_decorated_definition(
                    child,
                    &parsed.source,
                    &module_path,
                    None,
                    &mut symbols,
                );
            }
            _ => {}
        }
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
    }
}

fn collect_decorated_definition(
    node: Node,
    source: &str,
    module_path: &str,
    parent: Option<&str>,
    symbols: &mut Vec<SymbolAst>,
) {
    let decorators = named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "decorator")
        .map(|decorator| DecoratorAst {
            text: text(decorator, source)
                .trim_start_matches('@')
                .trim()
                .to_string(),
            source_span: span(decorator),
        })
        .collect::<Vec<_>>();

    for child in named_children(node) {
        match child.kind() {
            "class_definition" => {
                collect_class(child, source, module_path, decorators.clone(), symbols);
            }
            "function_definition" => {
                collect_function(
                    child,
                    source,
                    module_path,
                    parent,
                    decorators.clone(),
                    symbols,
                );
            }
            _ => {}
        }
    }
}

fn collect_class(
    node: Node,
    source: &str,
    module_path: &str,
    decorators: Vec<DecoratorAst>,
    symbols: &mut Vec<SymbolAst>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text(name_node, source);
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
        decorators,
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

    if let Some(body) = node.child_by_field_name("body") {
        for child in named_children(body) {
            match child.kind() {
                "function_definition" => {
                    collect_function(child, source, module_path, Some(&name), Vec::new(), symbols);
                }
                "decorated_definition" => {
                    collect_decorated_definition(child, source, module_path, Some(&name), symbols);
                }
                _ => {}
            }
        }
    }
}

fn collect_function(
    node: Node,
    source: &str,
    module_path: &str,
    parent: Option<&str>,
    decorators: Vec<DecoratorAst>,
    symbols: &mut Vec<SymbolAst>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text(name_node, source);
    let qualified_name = parent
        .map(|parent| format!("{parent}.{name}"))
        .unwrap_or_else(|| name.clone());
    let parameters = node
        .child_by_field_name("parameters")
        .map(|parameters| params_in(parameters, source))
        .unwrap_or_default();
    let return_type = node
        .child_by_field_name("return_type")
        .map(|return_type| text(return_type, source));
    let id = format!("{module_path}:{qualified_name}");
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
        decorators,
        return_type,
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
    let (module, names) = match node.kind() {
        "import_from_statement" => parse_from_import(&import_text),
        "import_statement" => (
            None,
            parse_import_names(import_text.trim_start_matches("import ")),
        ),
        _ => (None, Vec::new()),
    };

    ImportAst {
        text: import_text,
        module,
        names,
        source_span: span(node),
    }
}

fn parse_from_import(import_text: &str) -> (Option<String>, Vec<ImportNameAst>) {
    let Some(rest) = import_text.strip_prefix("from ") else {
        return (None, Vec::new());
    };
    let Some((module, names)) = rest.split_once(" import ") else {
        return (None, Vec::new());
    };

    (Some(module.trim().to_string()), parse_import_names(names))
}

fn parse_import_names(names: &str) -> Vec<ImportNameAst> {
    names
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let (name, alias) = name
                .split_once(" as ")
                .map(|(name, alias)| (name.trim(), Some(alias.trim().to_string())))
                .unwrap_or((name, None));
            ImportNameAst {
                name: name.to_string(),
                alias,
            }
        })
        .collect()
}

fn extract_assignment(node: Node, source: &str) -> Option<AssignmentAst> {
    let assignment = named_children(node)
        .into_iter()
        .find(|child| matches!(child.kind(), "assignment" | "type_alias_statement"))?;
    let target = assignment
        .child_by_field_name("left")
        .or_else(|| named_children(assignment).into_iter().next())
        .map(|target| text(target, source))?;

    Some(AssignmentAst {
        target,
        value: assignment
            .child_by_field_name("right")
            .map(|value| text(value, source)),
        text: text(assignment, source),
        source_span: span(assignment),
    })
}

fn assignments_in(node: Node, source: &str) -> Vec<AssignmentAst> {
    let mut assignments = Vec::new();
    visit_named_descendants(node, &mut |descendant| {
        if matches!(descendant.kind(), "expression_statement") {
            assignments.extend(extract_assignment(descendant, source));
        }
    });
    assignments
}

fn params_in(node: Node, source: &str) -> Vec<ParamAst> {
    let mut parameters = named_children(node)
        .into_iter()
        .filter_map(|child| {
            let name_node = match child.kind() {
                "identifier" => child,
                "typed_parameter" | "default_parameter" | "typed_default_parameter" => {
                    child.child_by_field_name("name")?
                }
                _ => return None,
            };
            Some(ParamAst {
                name: text(name_node, source),
                text: text(child, source),
                source_span: span(child),
            })
        })
        .collect::<Vec<_>>();

    for parameter_text in text(node, source)
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(str::trim)
        .filter(|parameter| !parameter.is_empty())
    {
        let name_without_type = parameter_text
            .split_once(':')
            .map(|(name, _)| name)
            .unwrap_or(parameter_text);
        let name = name_without_type
            .split_once('=')
            .map(|(name, _)| name)
            .unwrap_or(name_without_type)
            .trim()
            .trim_start_matches('*')
            .to_string();
        if name.is_empty() || parameters.iter().any(|parameter| parameter.name == name) {
            continue;
        }
        parameters.push(ParamAst {
            name,
            text: parameter_text.to_string(),
            source_span: span(node),
        });
    }

    parameters
}

fn calls_in(node: Node, source: &str, context: CallContext) -> Vec<CallAst> {
    let mut calls = Vec::new();
    visit_named_descendants(node, &mut |descendant| {
        if descendant.kind() != "call" {
            return;
        }

        let Some(function) = descendant.child_by_field_name("function") else {
            return;
        };
        let callee = text(function, source);
        calls.push(CallAst {
            receiver: receiver_for(function, source),
            callee,
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
    });
    calls
}

fn decorator_calls(node: Node, source: &str) -> Vec<CallAst> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "decorator")
        .flat_map(|decorator| calls_in(decorator, source, CallContext::Decorator))
        .collect()
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
        if descendant.kind() == "raise_statement" {
            facts.raises.push(RaiseAst {
                text: text(descendant, source),
                value: first_named_child(descendant).map(|value| text(value, source)),
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
        if descendant.kind() == "attribute" {
            facts
                .field_accesses
                .push(field_access_fact(descendant, source, owner_id));
        }
        if descendant.kind() == "subscript" {
            facts
                .index_accesses
                .push(index_access_fact(descendant, source, owner_id));
        }
    });
    facts
}

fn statement_fact(node: Node, source: &str, owner_id: &str) -> Option<StatementAst> {
    let kind = match node.kind() {
        "import_statement" | "import_from_statement" => StatementKind::Declaration,
        "expression_statement" if expression_statement_is_assignment(node) => {
            StatementKind::Assignment
        }
        "expression_statement" => StatementKind::Expression,
        "return_statement" => StatementKind::Return,
        "raise_statement" => StatementKind::Raise,
        "if_statement" | "elif_clause" => StatementKind::If,
        "for_statement" | "while_statement" => StatementKind::Loop,
        "function_definition" => StatementKind::Function,
        "class_definition" => StatementKind::Class,
        "decorated_definition" => named_children(node)
            .into_iter()
            .find_map(|child| match child.kind() {
                "function_definition" => Some(StatementKind::Function),
                "class_definition" => Some(StatementKind::Class),
                _ => None,
            })
            .unwrap_or(StatementKind::Unknown),
        "break_statement" => StatementKind::Break,
        "continue_statement" => StatementKind::Continue,
        "try_statement" => StatementKind::Try,
        "except_clause" => StatementKind::Catch,
        "finally_clause" => StatementKind::Finally,
        "assert_statement" | "delete_statement" | "global_statement" | "nonlocal_statement"
        | "pass_statement" | "with_statement" | "match_statement" => StatementKind::Unknown,
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
        "elif_clause" => (
            ConditionKind::ElseIf,
            node.child_by_field_name("condition")?,
        ),
        "while_statement" => (ConditionKind::While, node.child_by_field_name("condition")?),
        "for_statement" => (ConditionKind::For, node.child_by_field_name("left")?),
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
    if node.kind() != "return_statement" {
        return None;
    }
    Some(ReturnAst {
        value: first_named_child(node).map(|value| text(value, source)),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    })
}

fn definitions_for_node(node: Node, source: &str, owner_id: &str) -> Vec<DefinitionAst> {
    if node.kind() != "assignment" && node.kind() != "type_alias_statement" {
        return Vec::new();
    }

    let Some(target) = node
        .child_by_field_name("left")
        .or_else(|| first_named_child(node))
    else {
        return Vec::new();
    };

    vec![DefinitionAst {
        name: definition_name(target, source),
        kind: definition_kind(target),
        text: text(target, source),
        owner_id: owner_id.to_string(),
        source_span: span(target),
    }]
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
        "identifier" => ExpressionKind::Identifier,
        "string" | "integer" | "float" | "true" | "false" | "none" => ExpressionKind::Literal,
        "call" => ExpressionKind::Call,
        "attribute" => ExpressionKind::FieldAccess,
        "subscript" => ExpressionKind::IndexAccess,
        "assignment" => ExpressionKind::Assignment,
        "binary_operator" | "boolean_operator" | "comparison_operator" => {
            ExpressionKind::BinaryOperator
        }
        "unary_operator" | "not_operator" => ExpressionKind::UnaryOperator,
        "conditional_expression" => ExpressionKind::Conditional,
        "await" => ExpressionKind::Await,
        "yield" | "yield_from" => ExpressionKind::Yield,
        "list_comprehension"
        | "set_comprehension"
        | "dictionary_comprehension"
        | "generator_expression" => ExpressionKind::Unknown,
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
            .child_by_field_name("attribute")
            .map(|attribute| text(attribute, source))
            .unwrap_or_else(|| text(node, source)),
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    }
}

fn index_access_fact(node: Node, source: &str, owner_id: &str) -> IndexAccessAst {
    IndexAccessAst {
        object: node
            .child_by_field_name("value")
            .or_else(|| first_named_child(node))
            .map(|object| text(object, source)),
        index: node
            .child_by_field_name("subscript")
            .map(|index| text(index, source)),
        text: text(node, source),
        owner_id: owner_id.to_string(),
        source_span: span(node),
    }
}

fn definition_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => text(node, source),
        "attribute" => node
            .child_by_field_name("attribute")
            .map(|attribute| text(attribute, source))
            .unwrap_or_else(|| text(node, source)),
        _ => text(node, source),
    }
}

fn definition_kind(node: Node) -> DefinitionKind {
    match node.kind() {
        "identifier" => DefinitionKind::Variable,
        "attribute" => DefinitionKind::Field,
        _ => DefinitionKind::Unknown,
    }
}

fn expression_statement_is_assignment(node: Node) -> bool {
    named_children(node)
        .into_iter()
        .any(|child| matches!(child.kind(), "assignment" | "type_alias_statement"))
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
        "class_definition" | "function_definition" | "decorated_definition"
    )
}

fn receiver_for(node: Node, source: &str) -> Option<String> {
    if node.kind() != "attribute" {
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

fn argument_names(node: Node, source: &str) -> Vec<String> {
    named_children(node)
        .into_iter()
        .filter_map(|child| {
            if child.kind() != "keyword_argument" {
                return None;
            }
            child
                .child_by_field_name("name")
                .map(|name| text(name, source))
        })
        .collect()
}

fn module_path(relative_path: &str) -> String {
    relative_path
        .trim_end_matches(".py")
        .trim_end_matches("/__init__")
        .replace('/', ".")
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn first_named_child(node: Node) -> Option<Node> {
    named_children(node).into_iter().next()
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

    #[test]
    fn sg020_python_parser_emits_required_statement_kinds() {
        let parsed = parse_python_tree(
            "sample.py".to_string(),
            r#"
import os

class Example:
    def method(self):
        pass

def run(value):
    result = value
    effect(result)
    if result:
        return result
    while result:
        raise RuntimeError()
    pass
"#
            .to_string(),
        )
        .expect("parse python fixture");
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
            StatementKind::Raise,
            StatementKind::Function,
            StatementKind::Class,
            StatementKind::Unknown,
        ] {
            assert!(
                kinds.contains(&kind),
                "missing Python statement kind {kind:?}"
            );
        }
    }

    #[test]
    fn sg021_python_parser_emits_required_expression_kinds() {
        let parsed = parse_python_tree(
            "sample.py".to_string(),
            r#"
async def run(obj, items, index, value):
    result = value + 1
    send(value, obj.field, items[index], 42)
    choice = value if result > 0 else 0
    await send(value)
    yield result
"#
            .to_string(),
        )
        .expect("parse Python expression fixture");
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
            ExpressionKind::Conditional,
            ExpressionKind::Await,
            ExpressionKind::Yield,
        ] {
            assert!(
                kinds.contains(&kind),
                "missing Python expression kind {kind:?}"
            );
        }
    }
}
