//! Loads a supergraph snapshot (v2 or v3, compact or expanded) and rewrites it.
//!
//! usage: convert <input.json> <output.json> [--expand-strings]
use std::io::BufWriter;

use supergraph::supergraph::ProgramSupergraph;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("input path");
    let output = args.next().expect("output path");
    let expand = args.any(|arg| arg == "--expand-strings");
    let graph: ProgramSupergraph = serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(input)?))?;
    let mut writer = BufWriter::new(std::fs::File::create(output)?);
    if expand {
        graph.write_json_expanded(&mut writer)?;
    } else {
        graph.write_json(&mut writer)?;
    }
    Ok(())
}
