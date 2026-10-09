use std::collections::BTreeMap;
use std::fs;

use supergraph::analyze_rust_supergraph;
use supergraph::supergraph::NodeFact;

const SOURCE: &str = r#"
struct Answers { answers: Vec<Option<u32>> }

pub fn show(items: &Answers, quiet: bool) -> String {
    let mut out = String::new();
    if quiet {
        out.push_str("quiet");
    } else {
        for a in &items.answers {
            let picked = a
                .map(|i| format!("option {i}"))
                .unwrap_or_else(|| "no option".to_string());
            let text = a.map(|t| format!(": {t}")).unwrap_or_default();
            out.push_str(&format!("{picked}{text}"));
        }
    }
    out
}
"#;

/// A loop's body and continuation regions cover the same span. They used to produce two
/// sibling scopes with identical spans, so which one a loop variable bound into (and which one
/// its uses looked from) depended on the order of the hashed scope ids, and uses of the loop
/// variable silently went unresolved for some id assignments.
#[test]
fn loop_regions_do_not_produce_identical_sibling_scopes() {
    let dir = std::env::temp_dir().join(format!("supergraph-scope-res-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("lib.rs"), SOURCE).unwrap();
    let graph = analyze_rust_supergraph(&dir).expect("analysis");
    let _ = fs::remove_dir_all(&dir);

    let mut scopes_by_span = BTreeMap::new();
    for node in &graph.nodes {
        if let (NodeFact::Scope(scope), Some(span)) = (&node.fact, node.span) {
            *scopes_by_span
                .entry((node.owner.artifact_id, span, format!("{:?}", scope.kind)))
                .or_insert(0usize) += 1;
        }
    }
    assert!(
        scopes_by_span.values().all(|count| *count == 1),
        "scopes with an identical span and kind: {:?}",
        scopes_by_span.iter().filter(|(_, count)| **count > 1).collect::<Vec<_>>()
    );
}
