//! Loads a v3 supergraph snapshot (compact or expanded) and rewrites it or counts its facts.
//!
//! usage: convert <input.json> <output.json> [--expand-strings]
//!        convert <input.json> --counts
use std::collections::BTreeMap;
use std::io::BufWriter;

use supergraph::supergraph::ProgramSupergraph;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("input path");
    let second = args.next().expect("output path or --counts");
    let graph: ProgramSupergraph =
        serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(input)?))?;
    if second == "--counts" {
        let mut nodes = BTreeMap::<String, usize>::new();
        let mut edges = BTreeMap::<String, usize>::new();
        for node in &graph.nodes {
            *nodes.entry(format!("{:?}", node.kind)).or_default() += 1;
        }
        for edge in &graph.edges {
            *edges.entry(format!("{:?}", edge.kind)).or_default() += 1;
        }
        println!("nodes {} edges {}", graph.nodes.len(), graph.edges.len());
        for (kind, count) in nodes {
            println!("node {kind} {count}");
        }
        for (kind, count) in edges {
            println!("edge {kind} {count}");
        }
        return Ok(());
    }
    let expand = args.any(|arg| arg == "--expand-strings");
    let mut writer = BufWriter::new(std::fs::File::create(second)?);
    if expand {
        graph.write_json_expanded(&mut writer)?;
    } else {
        graph.write_json(&mut writer)?;
    }
    Ok(())
}
