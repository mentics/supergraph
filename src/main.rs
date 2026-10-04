use std::fs;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use supergraph::{
    analyze_python_path, analyze_python_supergraph, analyze_rust_path, analyze_rust_supergraph,
    analyze_typescript_path, analyze_typescript_supergraph,
};
use serde::Serialize;
use supergraph::supergraph::ProgramSupergraph;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "supergraph", about = "Build program supergraphs from source code")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Language {
    Python,
    Rust,
    Typescript,
}

#[derive(Subcommand)]
enum Command {
    /// Build a supergraph (JSON) from a source file or directory.
    Build {
        /// Source language to analyze.
        #[arg(value_enum)]
        language: Language,
        /// File or directory to analyze.
        path: PathBuf,
        /// Write JSON to this file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Pretty-print the JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Dump the per-file project AST (JSON) without building the supergraph.
    Ast {
        #[arg(value_enum)]
        language: Language,
        path: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        pretty: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Build { language, path, output, pretty } => {
            let graph = match language {
                Language::Python => analyze_python_supergraph(&path)?,
                Language::Rust => analyze_rust_supergraph(&path)?,
                Language::Typescript => analyze_typescript_supergraph(&path)?,
            };
            let r = emit_graph(&graph, output, pretty);
            // The process is about to exit; skip freeing the (very large) graph.
            std::mem::forget(graph);
            r
        }
        Command::Ast { language, path, output, pretty } => {
            let project = match language {
                Language::Python => analyze_python_path(&path)?,
                Language::Rust => analyze_rust_path(&path)?,
                Language::Typescript => analyze_typescript_path(&path)?,
            };
            emit(&project, output, pretty)
        }
    }
}

fn emit<T: Serialize>(value: &T, output: Option<PathBuf>, pretty: bool) -> Result<()> {
    let json = if pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    match output {
        Some(path) => fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display())),
        None => {
            println!("{json}");
            Ok(())
        }
    }
}

fn emit_graph(graph: &ProgramSupergraph, output: Option<PathBuf>, pretty: bool) -> Result<()> {
    if pretty {
        return emit(graph, output, pretty);
    }
    match output {
        Some(path) => {
            let file = fs::File::create(&path)
                .with_context(|| format!("failed to write {}", path.display()))?;
            let mut writer = BufWriter::with_capacity(1 << 20, file);
            graph.write_json(&mut writer)?;
            writer.flush()?;
            Ok(())
        }
        None => {
            let stdout = std::io::stdout();
            let mut writer = BufWriter::with_capacity(1 << 20, stdout.lock());
            graph.write_json(&mut writer)?;
            writeln!(writer)?;
            writer.flush()?;
            Ok(())
        }
    }
}
