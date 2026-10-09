pub mod python;
pub mod rust;
pub mod typescript;

use tree_sitter::Node;

use crate::ast::{ParseErrorAst, ParseErrorKind, SourceSpan};

/// Longest source excerpt kept in a parse error.
const MAX_ERROR_TEXT_CHARS: usize = 120;

/// Collects the unparseable (`ERROR`) and omitted (`MISSING`) regions of a syntax tree.
///
/// Only subtrees that report `has_error` are visited, so a clean file costs one check. The
/// children of an `ERROR` node are not reported again.
pub(crate) fn collect_parse_errors(root: Node, source: &str) -> Vec<ParseErrorAst> {
    let mut errors = Vec::new();
    if root.has_error() {
        visit_errors(root, source, &mut errors);
    }
    errors
}

fn visit_errors(node: Node, source: &str, errors: &mut Vec<ParseErrorAst>) {
    if node.is_missing() {
        let expected = if node.kind().is_empty() { "token" } else { node.kind() };
        errors.push(ParseErrorAst {
            kind: ParseErrorKind::Missing,
            message: format!("missing {expected}"),
            text: String::new(),
            source_span: span_of(node),
        });
        return;
    }
    if node.is_error() {
        let text = excerpt(node, source);
        errors.push(ParseErrorAst {
            kind: ParseErrorKind::Unparseable,
            message: format!("syntax error: could not parse `{text}`"),
            text,
            source_span: span_of(node),
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.has_error() || child.is_missing() {
            visit_errors(child, source, errors);
        }
    }
}

fn excerpt(node: Node, source: &str) -> String {
    let text = source.get(node.byte_range()).unwrap_or_default();
    let first_line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut excerpt = first_line
        .chars()
        .take(MAX_ERROR_TEXT_CHARS)
        .collect::<String>();
    if first_line.chars().count() > MAX_ERROR_TEXT_CHARS || text.lines().filter(|line| !line.trim().is_empty()).nth(1).is_some() {
        excerpt.push('…');
    }
    excerpt
}

fn span_of(node: Node) -> SourceSpan {
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
