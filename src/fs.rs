use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::{DirEntry, WalkBuilder};

/// Directories that never hold project source: VCS metadata, virtual environments,
/// dependency trees and build or tool caches.
const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    ".tox",
    ".nox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "__pycache__",
    "node_modules",
    "target",
];

/// Walk filter that prunes `SKIPPED_DIRECTORIES` below the analysis root. The root itself is
/// always walked, so analyzing a directory that is named like a skipped one still works.
fn should_descend(entry: &DirEntry) -> bool {
    entry.depth() == 0
        || !entry.file_type().is_some_and(|file_type| file_type.is_dir())
        || !SKIPPED_DIRECTORIES
            .iter()
            .any(|skipped| entry.file_name() == *skipped)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonSourceFile {
    pub absolute_path: PathBuf,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScriptSourceFile {
    pub absolute_path: PathBuf,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustSourceFile {
    pub absolute_path: PathBuf,
    pub relative_path: String,
}

pub fn discover_python_files(root: &Path) -> Result<Vec<PythonSourceFile>> {
    Ok(discover(root, None, |path| has_extension(path, &["py"]))?
        .into_iter()
        .map(|(absolute_path, relative_path)| PythonSourceFile { absolute_path, relative_path })
        .collect())
}

pub fn discover_typescript_files(root: &Path) -> Result<Vec<TypeScriptSourceFile>> {
    Ok(discover(root, None, is_typescript_source)?
        .into_iter()
        .map(|(absolute_path, relative_path)| TypeScriptSourceFile { absolute_path, relative_path })
        .collect())
}

pub fn discover_rust_files(root: &Path) -> Result<Vec<RustSourceFile>> {
    Ok(discover(root, None, |path| has_extension(path, &["rs"]))?
        .into_iter()
        .map(|(absolute_path, relative_path)| RustSourceFile { absolute_path, relative_path })
        .collect())
}

/// Walks `root` and returns `(absolute, root-relative)` paths of the files `wanted` accepts,
/// sorted by relative path. Honors `.gitignore`, `.git/info/exclude` and the global gitignore
/// (also outside a git repository), and does not descend into hidden directories.
fn discover(
    root: &Path,
    max_depth: Option<usize>,
    wanted: impl Fn(&Path) -> bool,
) -> Result<Vec<(PathBuf, String)>> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize analysis root {}", root.display()))?;

    let mut files = Vec::new();
    for entry in WalkBuilder::new(&root)
        .require_git(false)
        .max_depth(max_depth)
        .sort_by_file_name(|left, right| left.cmp(right))
        .filter_entry(should_descend)
        .build()
    {
        let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
        if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
            continue;
        }

        let path = entry.into_path();
        if !wanted(&path) {
            continue;
        }

        let relative_path = path
            .strip_prefix(&root)
            .with_context(|| {
                format!(
                    "failed to make {} relative to {}",
                    path.display(),
                    root.display()
                )
            })?
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        files.push((path, relative_path));
    }

    files.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(files)
}

/// Finds files named exactly one of `names` under `root`, as `(absolute, root-relative)` paths.
/// `max_depth` of `Some(1)` looks only at the directory itself.
pub fn discover_files_named(
    root: &Path,
    names: &[&str],
    max_depth: Option<usize>,
) -> Result<Vec<(PathBuf, String)>> {
    discover(root, max_depth, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| names.contains(&name))
    })
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension))
}

fn is_typescript_source(path: &Path) -> bool {
    has_extension(path, &["ts", "tsx"])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn discovery_skips_environment_build_hidden_and_gitignored_directories() {
        let root = std::env::temp_dir().join(format!("supergraph-fs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "pkg",
            ".venv/lib",
            "venv/lib",
            "target/debug",
            ".git/hooks",
            "node_modules/dep",
            "pkg/__pycache__",
            ".claude/worktrees/w",
            "ignored",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "pkg/a.py",
            ".venv/lib/b.py",
            "venv/lib/c.py",
            "target/debug/d.py",
            ".git/hooks/e.py",
            "node_modules/dep/f.py",
            "pkg/__pycache__/g.py",
            "pkg/a.ts",
            "node_modules/dep/h.ts",
            "pkg/lib.rs",
            "target/debug/i.rs",
            ".claude/worktrees/w/j.py",
            "ignored/k.py",
            ".gitignore",
        ] {
            let contents = if file == ".gitignore" { "ignored/
" } else { "" };
            fs::write(root.join(file), contents).unwrap();
        }

        let python = discover_python_files(&root).unwrap();
        assert_eq!(
            python.iter().map(|f| f.relative_path.as_str()).collect::<Vec<_>>(),
            ["pkg/a.py"]
        );
        let typescript = discover_typescript_files(&root).unwrap();
        assert_eq!(typescript.len(), 1);
        let rust = discover_rust_files(&root).unwrap();
        assert_eq!(rust.len(), 1);

        // A root that is itself named like a skipped directory is still analyzed.
        let nested_root = root.join("venv/lib");
        assert_eq!(discover_python_files(&nested_root).unwrap().len(), 1);

        fs::remove_dir_all(&root).unwrap();
    }
}
