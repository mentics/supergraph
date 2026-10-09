use std::fs;
use std::path::PathBuf;

use supergraph::supergraph::{DiagnosticKind, NodeFact, ProgramSupergraph, Severity};
use supergraph::{analyze_python_supergraph, analyze_rust_supergraph, analyze_typescript_supergraph};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("supergraph-parse-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn parse_error_diagnostics(graph: &ProgramSupergraph) -> Vec<&supergraph::supergraph::Diagnostic> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Diagnostic(diagnostic) if diagnostic.kind == DiagnosticKind::ParseError => {
                Some(diagnostic)
            }
            _ => None,
        })
        .collect()
}

fn has_callable(graph: &ProgramSupergraph, name: &str) -> bool {
    graph.nodes.iter().any(|node| match &node.fact {
        NodeFact::Callable(callable) => callable.qualified_name.ends_with(name),
        _ => false,
    })
}

#[test]
fn python_keeps_parseable_code_and_reports_the_broken_region() {
    let dir = scratch_dir("py");
    fs::write(dir.join("ok.py"), "def fine():\n    return 1\n").unwrap();
    fs::write(
        dir.join("broken.py"),
        "def good(a):\n    return a + 1\n\ndef bad(:\n    x = = 3\n\ndef also_good():\n    return good(2)\n",
    )
    .unwrap();

    let graph = analyze_python_supergraph(&dir).expect("broken file must not abort analysis");

    assert!(has_callable(&graph, "ok.fine"));
    assert!(has_callable(&graph, "broken.good"));
    let errors = parse_error_diagnostics(&graph);
    assert!(!errors.is_empty());
    for error in &errors {
        assert_eq!(error.severity, Severity::Error);
        assert!(error.message.starts_with("broken.py:"), "{}", error.message);
        assert!(error.artifact_id.is_some());
        assert!(error.span.is_some());
    }
    assert!(
        errors.iter().all(|error| !error.message.starts_with("ok.py")),
        "clean files must not get parse errors"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn typescript_keeps_parseable_code_and_reports_the_broken_region() {
    let dir = scratch_dir("ts");
    fs::write(
        dir.join("m.ts"),
        "function good(a: number) { return a + 1; }\nfunction bad( { let x = ;\n",
    )
    .unwrap();

    let graph = analyze_typescript_supergraph(&dir).expect("broken file must not abort analysis");

    assert!(has_callable(&graph, "m.good"));
    let errors = parse_error_diagnostics(&graph);
    assert!(!errors.is_empty());
    assert!(errors[0].message.starts_with("m.ts:2:"), "{}", errors[0].message);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rust_keeps_parseable_code_and_reports_the_broken_region() {
    let dir = scratch_dir("rs");
    fs::write(
        dir.join("m.rs"),
        "fn good(a: i32) -> i32 { a + 1 }\nfn bad( { let x = ;\n",
    )
    .unwrap();

    let graph = analyze_rust_supergraph(&dir).expect("broken file must not abort analysis");

    assert!(has_callable(&graph, "m.good"));
    let errors = parse_error_diagnostics(&graph);
    assert!(!errors.is_empty());
    assert!(errors[0].message.starts_with("m.rs:"), "{}", errors[0].message);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn clean_files_produce_no_parse_errors() {
    let dir = scratch_dir("clean");
    fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
    let graph = analyze_python_supergraph(&dir).unwrap();
    assert!(parse_error_diagnostics(&graph).is_empty());
    fs::remove_dir_all(&dir).unwrap();
}
