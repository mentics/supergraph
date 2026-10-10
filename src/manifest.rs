//! Discovery and parsing of package-manager manifests.
//!
//! Each language analyzed has one ecosystem: Rust uses Cargo, TypeScript uses npm (and pnpm
//! workspaces), Python uses `pyproject.toml`, `setup.cfg` and `setup.py`. Manifests are looked
//! for under the analysis root and in the directories enclosing it, up to the repository root
//! (the nearest directory holding `.git`). Without a repository boundary only the root itself
//! is searched, so unrelated manifests higher in the filesystem are never picked up.
//!
//! A manifest that cannot be parsed is skipped rather than failing the analysis.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use serde_json::Value as Json;
use toml::{Table, Value as Toml};

use crate::ast::{
    DependencyAst, DependencyKind, Ecosystem, ManifestAst, ManifestFormat, PackageAst, TargetAst, TargetKind,
    WorkspaceAst,
};
use crate::fs::discover_files_named;

fn file_names(ecosystem: Ecosystem) -> &'static [&'static str] {
    match ecosystem {
        Ecosystem::Cargo => &["Cargo.toml"],
        Ecosystem::Npm => &["package.json", "pnpm-workspace.yaml"],
        Ecosystem::Python => &["pyproject.toml", "setup.cfg", "setup.py"],
    }
}

struct Parsed {
    abs_dir: PathBuf,
    manifest: ManifestAst,
}

/// Finds and parses the manifests of `ecosystem` for the analysis root `root` (a directory).
/// With `shallow`, only manifests directly inside `root` are taken from below it, which is what
/// analyzing a single file wants.
pub(crate) fn discover_manifests(
    root: &Path,
    ecosystem: Ecosystem,
    shallow: bool,
) -> Result<Vec<ManifestAst>> {
    let names = file_names(ecosystem);
    let mut found = discover_files_named(root, names, shallow.then_some(1))?
        .into_iter()
        .map(|(absolute, _)| absolute)
        .collect::<Vec<_>>();
    for dir in enclosing_dirs(root) {
        found.extend(names.iter().map(|name| dir.join(name)).filter(|path| path.is_file()));
    }

    let mut parsed = found
        .iter()
        .filter_map(|path| parse_manifest(root, path))
        .collect::<Vec<_>>();
    if ecosystem == Ecosystem::Python {
        keep_highest_priority_python_package(&mut parsed);
    }
    assign_workspaces(&mut parsed);

    let mut manifests = parsed.into_iter().map(|parsed| parsed.manifest).collect::<Vec<_>>();
    manifests.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(manifests)
}

/// Directories above `root`, nearest first, up to and including the repository root.
fn enclosing_dirs(root: &Path) -> Vec<PathBuf> {
    if root.join(".git").exists() {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    for ancestor in root.ancestors().skip(1) {
        dirs.push(ancestor.to_path_buf());
        if ancestor.join(".git").exists() {
            return dirs;
        }
    }
    Vec::new()
}

fn parse_manifest(root: &Path, path: &Path) -> Option<Parsed> {
    let text = std::fs::read_to_string(path).ok()?;
    let abs_dir = path.parent()?.to_path_buf();
    let file_name = path.file_name()?.to_str()?;
    let (format, package, workspace) = match file_name {
        "Cargo.toml" => {
            let (package, workspace) = parse_cargo(root, &abs_dir, &text)?;
            (ManifestFormat::CargoToml, package, workspace)
        }
        "package.json" => {
            let (package, workspace) = parse_package_json(root, &abs_dir, &text)?;
            (ManifestFormat::PackageJson, package, workspace)
        }
        "pnpm-workspace.yaml" => {
            (ManifestFormat::PnpmWorkspaceYaml, None, Some(parse_pnpm_workspace(&text)))
        }
        "pyproject.toml" => {
            let (package, workspace) = parse_pyproject(root, &abs_dir, &text)?;
            (ManifestFormat::PyprojectToml, package, workspace)
        }
        "setup.cfg" => (ManifestFormat::SetupCfg, parse_setup_cfg(&text), None),
        "setup.py" => (ManifestFormat::SetupPy, parse_setup_py(&text), None),
        _ => return None,
    };
    if package.is_none() && workspace.is_none() {
        return None;
    }
    Some(Parsed {
        manifest: ManifestAst {
            path: relative_to(root, path),
            dir: relative_to(root, &abs_dir),
            format,
            package,
            workspace,
        },
        abs_dir,
    })
}

/// A directory may carry `pyproject.toml`, `setup.cfg` and `setup.py` for the same project;
/// only the first of them (in that order) that declares a package keeps it.
fn keep_highest_priority_python_package(parsed: &mut Vec<Parsed>) {
    let priority = |format: ManifestFormat| match format {
        ManifestFormat::PyprojectToml => 0,
        ManifestFormat::SetupCfg => 1,
        _ => 2,
    };
    let mut best = std::collections::BTreeMap::<PathBuf, u8>::new();
    for parsed in parsed.iter().filter(|parsed| parsed.manifest.package.is_some()) {
        let entry = best.entry(parsed.abs_dir.clone()).or_insert(u8::MAX);
        *entry = (*entry).min(priority(parsed.manifest.format));
    }
    for parsed in parsed.iter_mut() {
        if best.get(&parsed.abs_dir).is_some_and(|best| *best < priority(parsed.manifest.format)) {
            parsed.manifest.package = None;
        }
    }
    parsed.retain(|parsed| parsed.manifest.package.is_some() || parsed.manifest.workspace.is_some());
}

/// Sets each package's `workspace_manifest` to the nearest workspace manifest at or above its
/// directory whose member patterns select it.
fn assign_workspaces(parsed: &mut [Parsed]) {
    let workspaces = parsed
        .iter()
        .filter_map(|parsed| {
            let workspace = parsed.manifest.workspace.as_ref()?;
            Some((parsed.abs_dir.clone(), parsed.manifest.path.clone(), workspace.clone()))
        })
        .collect::<Vec<_>>();
    for parsed in parsed.iter_mut() {
        let Some(package) = parsed.manifest.package.as_mut() else { continue };
        let mut best: Option<(usize, &str)> = None;
        for (workspace_dir, workspace_path, workspace) in &workspaces {
            let Ok(relative) = parsed.abs_dir.strip_prefix(workspace_dir) else { continue };
            // A package in the workspace's own directory is a member (including a root package
            // declared in the same manifest).
            let is_member = if relative.as_os_str().is_empty() {
                true
            } else {
                let relative = path_text(relative);
                selects(&workspace.members, &relative) && !selects(&workspace.exclude, &relative)
            };
            let depth = workspace_dir.components().count();
            if is_member && best.is_none_or(|(best_depth, _)| depth > best_depth) {
                best = Some((depth, workspace_path));
            }
        }
        package.workspace_manifest = best.map(|(_, path)| path.to_string());
    }
}

fn selects(patterns: &[String], relative_dir: &str) -> bool {
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
        glob_match(pattern, relative_dir)
    })
}

/// Matches a `/`-separated path against a pattern whose segments may use `*`, `?` and `**`.
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    fn segments(pattern: &[&str], path: &[&str]) -> bool {
        match pattern.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
            Some((segment, rest)) => path
                .split_first()
                .is_some_and(|(head, tail)| segment_match(segment, head) && segments(rest, tail)),
        }
    }
    fn segment_match(pattern: &str, text: &str) -> bool {
        fn go(pattern: &[char], text: &[char]) -> bool {
            match pattern.split_first() {
                None => text.is_empty(),
                Some(('*', rest)) => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
                Some(('?', rest)) => text.split_first().is_some_and(|(_, tail)| go(rest, tail)),
                Some((c, rest)) => text.split_first().is_some_and(|(t, tail)| t == c && go(rest, tail)),
            }
        }
        go(&pattern.chars().collect::<Vec<_>>(), &text.chars().collect::<Vec<_>>())
    }
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let path = path.split('/').collect::<Vec<_>>();
    segments(&pattern, &path)
}

// ---- paths ----------------------------------------------------------------------------------

fn path_text(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// `abs` as a `/`-separated path relative to `root`: `.` for the root, `..`-prefixed above it.
pub(crate) fn relative_to(root: &Path, abs: &Path) -> String {
    let abs = normalize(abs);
    for (up, ancestor) in root.ancestors().enumerate() {
        if let Ok(rest) = abs.strip_prefix(ancestor) {
            let mut parts = vec![".."; up];
            let rest = path_text(rest);
            if !rest.is_empty() {
                parts.push(&rest);
            }
            return if parts.is_empty() { ".".to_string() } else { parts.join("/") };
        }
    }
    path_text(&abs)
}

// ---- Cargo ----------------------------------------------------------------------------------

fn parse_cargo(root: &Path, dir: &Path, text: &str) -> Option<(Option<PackageAst>, Option<WorkspaceAst>)> {
    let table = text.parse::<Table>().ok()?;
    let workspace = table.get("workspace").and_then(Toml::as_table).map(|workspace| WorkspaceAst {
        members: string_array(workspace.get("members")),
        exclude: string_array(workspace.get("exclude")),
    });
    let package = table.get("package").and_then(Toml::as_table).and_then(|package| {
        let name = package.get("name")?.as_str()?.to_string();
        Some(PackageAst {
            version: package.get("version").and_then(Toml::as_str).map(str::to_string),
            targets: cargo_targets(root, dir, &table, package, &name),
            entry_files: Vec::new(),
            dependencies: cargo_dependencies(root, dir, &table),
            workspace_manifest: None,
            name,
        })
    });
    Some((package, workspace))
}

fn string_array(value: Option<&Toml>) -> Vec<String> {
    value
        .and_then(Toml::as_array)
        .map(|items| items.iter().filter_map(Toml::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn cargo_dependencies(root: &Path, dir: &Path, manifest: &Table) -> Vec<DependencyAst> {
    const SECTIONS: [(&str, DependencyKind); 3] = [
        ("dependencies", DependencyKind::Normal),
        ("dev-dependencies", DependencyKind::Dev),
        ("build-dependencies", DependencyKind::Build),
    ];
    let mut tables = Vec::new();
    for (section, kind) in SECTIONS {
        if let Some(table) = manifest.get(section).and_then(Toml::as_table) {
            tables.push((table, kind));
        }
    }
    // `[target.'cfg(..)'.dependencies]` and friends.
    if let Some(targets) = manifest.get("target").and_then(Toml::as_table) {
        for target in targets.values().filter_map(Toml::as_table) {
            for (section, kind) in SECTIONS {
                if let Some(table) = target.get(section).and_then(Toml::as_table) {
                    tables.push((table, kind));
                }
            }
        }
    }

    let mut dependencies = Vec::new();
    for (table, kind) in tables {
        for (key, value) in table {
            let mut dependency = new_dependency(key, kind);
            match value {
                Toml::String(version) => dependency.requirement = Some(version.clone()),
                Toml::Table(spec) => {
                    dependency.requirement = spec.get("version").and_then(Toml::as_str).map(str::to_string);
                    dependency.optional = spec.get("optional").and_then(Toml::as_bool).unwrap_or(false);
                    dependency.workspace = spec.get("workspace").and_then(Toml::as_bool).unwrap_or(false);
                    dependency.path = spec
                        .get("path")
                        .and_then(Toml::as_str)
                        .map(|path| relative_to(root, &dir.join(path)));
                    if let Some(package) = spec.get("package").and_then(Toml::as_str) {
                        dependency.rename = Some(key.clone());
                        dependency.name = package.to_string();
                    }
                }
                _ => {}
            }
            dependencies.push(dependency);
        }
    }
    dependencies
}

fn new_dependency(name: &str, kind: DependencyKind) -> DependencyAst {
    DependencyAst {
        name: name.to_string(),
        rename: None,
        requirement: None,
        kind,
        optional: false,
        path: None,
        workspace: false,
        group: None,
    }
}

fn cargo_targets(root: &Path, dir: &Path, manifest: &Table, package: &Table, package_name: &str) -> Vec<TargetAst> {
    let mut targets = Vec::<TargetAst>::new();
    let mut add = |name: String, kind: TargetKind, path: &str| {
        let root_path = relative_to(root, &dir.join(path));
        if !targets.iter().any(|target| target.root_path == root_path && target.kind == kind) {
            targets.push(TargetAst { name, kind, root_path });
        }
    };
    let auto = |key: &str| package.get(key).and_then(Toml::as_bool).unwrap_or(true);
    let default_lib_name = package_name.replace('-', "_");

    // Explicit targets first, so their names win over autodiscovered ones.
    if let Some(lib) = manifest.get("lib").and_then(Toml::as_table) {
        let name = lib.get("name").and_then(Toml::as_str).unwrap_or(&default_lib_name).to_string();
        let path = lib.get("path").and_then(Toml::as_str).unwrap_or("src/lib.rs");
        add(name, TargetKind::Lib, path);
    } else if dir.join("src/lib.rs").is_file() {
        add(default_lib_name, TargetKind::Lib, "src/lib.rs");
    }

    for (key, kind, folder, auto_key) in [
        ("bin", TargetKind::Bin, "src/bin", "autobins"),
        ("example", TargetKind::Example, "examples", "autoexamples"),
        ("test", TargetKind::Test, "tests", "autotests"),
        ("bench", TargetKind::Bench, "benches", "autobenches"),
    ] {
        for target in manifest.get(key).and_then(Toml::as_array).into_iter().flatten().filter_map(Toml::as_table) {
            let Some(name) = target.get("name").and_then(Toml::as_str) else { continue };
            let path = match target.get("path").and_then(Toml::as_str) {
                Some(path) => path.to_string(),
                None if kind == TargetKind::Bin && name == package_name && dir.join("src/main.rs").is_file() => {
                    "src/main.rs".to_string()
                }
                None => format!("{folder}/{name}.rs"),
            };
            add(name.to_string(), kind, &path);
        }
        if kind == TargetKind::Bin && auto(auto_key) && dir.join("src/main.rs").is_file() {
            add(package_name.to_string(), kind, "src/main.rs");
        }
        if auto(auto_key) {
            for (name, path) in autodiscover(dir, folder) {
                add(name, kind, &path);
            }
        }
    }
    targets.sort_by(|left, right| (left.kind, &left.name).cmp(&(right.kind, &right.name)));
    targets
}

/// `folder/<name>.rs` and `folder/<name>/main.rs` entries, as `(name, dir-relative path)`.
fn autodiscover(dir: &Path, folder: &str) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir.join(folder)) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            let stem = name.trim_end_matches(".rs").to_string();
            found.push((stem, format!("{folder}/{name}")));
        } else if path.join("main.rs").is_file() {
            found.push((name.clone(), format!("{folder}/{name}/main.rs")));
        }
    }
    found.sort();
    found
}

// ---- npm ------------------------------------------------------------------------------------

fn parse_package_json(root: &Path, dir: &Path, text: &str) -> Option<(Option<PackageAst>, Option<WorkspaceAst>)> {
    let json = serde_json::from_str::<Json>(text).ok()?;
    let object = json.as_object()?;

    let patterns = match object.get("workspaces") {
        Some(Json::Array(items)) => json_strings(items),
        Some(Json::Object(workspaces)) => workspaces
            .get("packages")
            .and_then(Json::as_array)
            .map(|items| json_strings(items))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let workspace = object.contains_key("workspaces").then(|| split_negated(patterns));

    let package = object.get("name").and_then(Json::as_str).map(|name| {
        let mut entry_files = Vec::new();
        for key in ["main", "module"] {
            if let Some(path) = object.get(key).and_then(Json::as_str) {
                entry_files.push(relative_to(root, &dir.join(path)));
            }
        }
        match object.get("bin") {
            Some(Json::String(path)) => entry_files.push(relative_to(root, &dir.join(path))),
            Some(Json::Object(bins)) => entry_files.extend(
                bins.values().filter_map(Json::as_str).map(|path| relative_to(root, &dir.join(path))),
            ),
            _ => {}
        }
        entry_files.dedup();

        let mut dependencies = Vec::new();
        for (section, kind) in [
            ("dependencies", DependencyKind::Normal),
            ("devDependencies", DependencyKind::Dev),
            ("peerDependencies", DependencyKind::Peer),
            ("optionalDependencies", DependencyKind::Optional),
        ] {
            for (key, value) in object.get(section).and_then(Json::as_object).into_iter().flatten() {
                dependencies.push(npm_dependency(root, dir, key, value.as_str().unwrap_or_default(), kind));
            }
        }
        PackageAst {
            name: name.to_string(),
            version: object.get("version").and_then(Json::as_str).map(str::to_string),
            targets: Vec::new(),
            entry_files,
            dependencies,
            workspace_manifest: None,
        }
    });
    Some((package, workspace))
}

fn json_strings(items: &[Json]) -> Vec<String> {
    items.iter().filter_map(Json::as_str).map(str::to_string).collect()
}

/// Splits `!`-negated workspace patterns into the exclude list.
fn split_negated(patterns: Vec<String>) -> WorkspaceAst {
    let (exclude, members) = patterns.into_iter().partition::<Vec<_>, _>(|pattern| pattern.starts_with('!'));
    WorkspaceAst {
        members,
        exclude: exclude.into_iter().map(|pattern| pattern[1..].to_string()).collect(),
    }
}

fn npm_dependency(root: &Path, dir: &Path, key: &str, spec: &str, kind: DependencyKind) -> DependencyAst {
    let mut dependency = new_dependency(key, kind);
    dependency.optional = kind == DependencyKind::Optional;
    if !spec.is_empty() {
        dependency.requirement = Some(spec.to_string());
    }
    if let Some(path) = spec.strip_prefix("file:").or_else(|| spec.strip_prefix("link:")) {
        dependency.path = Some(relative_to(root, &dir.join(path)));
    } else if spec.starts_with("workspace:") {
        dependency.workspace = true;
    } else if let Some(alias) = spec.strip_prefix("npm:") {
        // `npm:real-name@^1` installs `real-name` under the local name `key`.
        let name_end = alias.rfind('@').filter(|&at| at > 0).unwrap_or(alias.len());
        dependency.rename = Some(key.to_string());
        dependency.name = alias[..name_end].to_string();
        dependency.requirement = alias.get(name_end + 1..).map(str::to_string).filter(|v| !v.is_empty());
    }
    dependency
}

fn parse_pnpm_workspace(text: &str) -> WorkspaceAst {
    let mut patterns = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t', '-']) {
            in_packages = trimmed.trim_end() == "packages:";
            continue;
        }
        if in_packages && let Some(item) = trimmed.strip_prefix('-') {
            patterns.push(item.trim().trim_matches(['\'', '"']).to_string());
        }
    }
    split_negated(patterns)
}

// ---- Python ---------------------------------------------------------------------------------

fn parse_pyproject(root: &Path, dir: &Path, text: &str) -> Option<(Option<PackageAst>, Option<WorkspaceAst>)> {
    let table = text.parse::<Table>().ok()?;
    let project = table.get("project").and_then(Toml::as_table);
    let tool = table.get("tool").and_then(Toml::as_table);
    let poetry = tool.and_then(|tool| tool.get("poetry")).and_then(Toml::as_table);
    let uv = tool.and_then(|tool| tool.get("uv")).and_then(Toml::as_table);

    let workspace = uv
        .and_then(|uv| uv.get("workspace"))
        .and_then(Toml::as_table)
        .map(|workspace| WorkspaceAst {
            members: string_array(workspace.get("members")),
            exclude: string_array(workspace.get("exclude")),
        });

    let source = project.or(poetry);
    let name = source.and_then(|source| source.get("name")).and_then(Toml::as_str);
    let package = name.map(|name| {
        let mut dependencies = Vec::new();
        let mut add_specs = |specs: Option<&Toml>, kind, group: Option<&str>| {
            for spec in specs.and_then(Toml::as_array).into_iter().flatten().filter_map(Toml::as_str) {
                if let Some(mut dependency) = python_dependency(spec, kind) {
                    dependency.group = group.map(str::to_string);
                    dependencies.push(dependency);
                }
            }
        };
        if let Some(project) = project {
            add_specs(project.get("dependencies"), DependencyKind::Normal, None);
            for (group, specs) in project.get("optional-dependencies").and_then(Toml::as_table).into_iter().flatten() {
                add_specs(Some(specs), DependencyKind::Optional, Some(group));
            }
        }
        for (group, specs) in table.get("dependency-groups").and_then(Toml::as_table).into_iter().flatten() {
            add_specs(Some(specs), DependencyKind::Dev, Some(group));
        }
        add_specs(
            table.get("build-system").and_then(Toml::as_table).and_then(|build| build.get("requires")),
            DependencyKind::Build,
            None,
        );
        if let Some(poetry) = poetry {
            poetry_dependencies(root, dir, poetry.get("dependencies"), DependencyKind::Normal, None, &mut dependencies);
            poetry_dependencies(root, dir, poetry.get("dev-dependencies"), DependencyKind::Dev, None, &mut dependencies);
            for (group, spec) in poetry.get("group").and_then(Toml::as_table).into_iter().flatten() {
                let kind = if group == "dev" || group == "test" { DependencyKind::Dev } else { DependencyKind::Optional };
                poetry_dependencies(root, dir, spec.get("dependencies"), kind, Some(group), &mut dependencies);
            }
        }
        apply_uv_sources(root, dir, uv, &mut dependencies);

        PackageAst {
            name: name.to_string(),
            version: source.and_then(|source| source.get("version")).and_then(Toml::as_str).map(str::to_string),
            targets: Vec::new(),
            entry_files: Vec::new(),
            dependencies,
            workspace_manifest: None,
        }
    });
    Some((package, workspace))
}

/// Splits a PEP 508 requirement into name and version specifier. Environment markers and
/// extras are dropped.
fn python_dependency(spec: &str, kind: DependencyKind) -> Option<DependencyAst> {
    let spec = spec.split(';').next()?.trim();
    let name_end = spec
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        .unwrap_or(spec.len());
    let name = &spec[..name_end];
    if name.is_empty() {
        return None;
    }
    let mut rest = spec[name_end..].trim_start();
    if rest.starts_with('[') {
        rest = rest[rest.find(']')? + 1..].trim_start();
    }
    let rest = rest.trim().trim_matches(['(', ')']).trim();
    let mut dependency = new_dependency(name, kind);
    dependency.requirement = Some(rest.to_string()).filter(|rest| !rest.is_empty());
    Some(dependency)
}

fn poetry_dependencies(
    root: &Path,
    dir: &Path,
    table: Option<&Toml>,
    kind: DependencyKind,
    group: Option<&str>,
    out: &mut Vec<DependencyAst>,
) {
    for (name, value) in table.and_then(Toml::as_table).into_iter().flatten() {
        if name == "python" {
            continue;
        }
        let mut dependency = new_dependency(name, kind);
        dependency.group = group.map(str::to_string);
        match value {
            Toml::String(version) => dependency.requirement = Some(version.clone()),
            Toml::Table(spec) => {
                dependency.requirement = spec.get("version").and_then(Toml::as_str).map(str::to_string);
                dependency.optional = spec.get("optional").and_then(Toml::as_bool).unwrap_or(false);
                dependency.path = spec.get("path").and_then(Toml::as_str).map(|path| relative_to(root, &dir.join(path)));
            }
            _ => {}
        }
        out.push(dependency);
    }
}

/// `[tool.uv.sources]` marks dependencies that resolve to a local path or workspace member.
fn apply_uv_sources(root: &Path, dir: &Path, uv: Option<&Table>, dependencies: &mut [DependencyAst]) {
    let normalize = |name: &str| name.to_ascii_lowercase().replace(['_', '.'], "-");
    for (name, source) in uv.and_then(|uv| uv.get("sources")).and_then(Toml::as_table).into_iter().flatten() {
        let Some(source) = source.as_table() else { continue };
        for dependency in dependencies.iter_mut().filter(|dependency| normalize(&dependency.name) == normalize(name)) {
            dependency.workspace |= source.get("workspace").and_then(Toml::as_bool).unwrap_or(false);
            if let Some(path) = source.get("path").and_then(Toml::as_str) {
                dependency.path = Some(relative_to(root, &dir.join(path)));
            }
        }
    }
}

fn parse_setup_cfg(text: &str) -> Option<PackageAst> {
    let mut section = String::new();
    let mut key = String::new();
    let mut values = std::collections::BTreeMap::<(String, String), Vec<String>>::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            values.entry((section.clone(), key.clone())).or_default().push(trimmed.to_string());
        } else if let Some(name) = trimmed.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name.trim().to_string();
        } else if let Some((name, value)) = trimmed.split_once(['=', ':']) {
            key = name.trim().to_string();
            let value = value.trim();
            values.insert((section.clone(), key.clone()), if value.is_empty() { Vec::new() } else { vec![value.to_string()] });
        }
    }
    let single = |section: &str, key: &str| {
        values.get(&(section.to_string(), key.to_string())).and_then(|lines| lines.first()).cloned()
    };
    let name = single("metadata", "name")?;
    let mut dependencies = Vec::new();
    for spec in values.get(&("options".to_string(), "install_requires".to_string())).into_iter().flatten() {
        dependencies.extend(python_dependency(spec, DependencyKind::Normal));
    }
    for ((section, group), specs) in &values {
        if section == "options.extras_require" {
            for spec in specs {
                if let Some(mut dependency) = python_dependency(spec, DependencyKind::Optional) {
                    dependency.group = Some(group.clone());
                    dependencies.push(dependency);
                }
            }
        }
    }
    Some(PackageAst {
        name,
        version: single("metadata", "version").filter(|version| !version.contains(':')),
        targets: Vec::new(),
        entry_files: Vec::new(),
        dependencies,
        workspace_manifest: None,
    })
}

/// Best effort: reads the literal `name=` and `version=` keyword arguments of `setup(...)`.
fn parse_setup_py(text: &str) -> Option<PackageAst> {
    let name = keyword_string(text, "name")?;
    Some(PackageAst {
        name,
        version: keyword_string(text, "version"),
        targets: Vec::new(),
        entry_files: Vec::new(),
        dependencies: Vec::new(),
        workspace_manifest: None,
    })
}

fn keyword_string(text: &str, keyword: &str) -> Option<String> {
    let mut search = text;
    while let Some(at) = search.find(keyword) {
        let (before, after) = (&search[..at], &search[at + keyword.len()..]);
        let boundary = before.chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let value = after.trim_start().strip_prefix('=').map(str::trim_start);
        if boundary
            && let Some(value) = value
            && let Some(quote) = value.chars().next().filter(|c| matches!(c, '"' | '\''))
            && let Some(end) = value[1..].find(quote)
        {
            return Some(value[1..1 + end].to_string());
        }
        search = after;
    }
    None
}

/// Names of every directory that is an ancestor (inclusive) of `path`, innermost first, for
/// root-relative paths like `a/b/c.rs` (`a/b`, `a`, `.`).
pub(crate) fn ancestor_directories(path: &str) -> Vec<String> {
    let mut directories = Vec::new();
    let mut current = path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        directories.push(parent.to_string());
        current = parent;
    }
    directories.push(".".to_string());
    directories
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("supergraph-manifest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn glob_patterns_match_workspace_members() {
        assert!(glob_match("crates/*", "crates/a"));
        assert!(!glob_match("crates/*", "crates/a/b"));
        assert!(glob_match("packages/**", "packages/a/b"));
        assert!(glob_match("pkg-?", "pkg-1"));
        assert!(!glob_match("crates/*", "other/a"));
    }

    #[test]
    fn python_requirements_split_name_from_specifier() {
        let dependency = python_dependency("requests[socks] >=2.0, <3 ; python_version > '3.8'", DependencyKind::Normal).unwrap();
        assert_eq!(dependency.name, "requests");
        assert_eq!(dependency.requirement.as_deref(), Some(">=2.0, <3"));
        assert_eq!(python_dependency("flask", DependencyKind::Normal).unwrap().requirement, None);
    }

    #[test]
    fn setup_py_keywords_are_read() {
        let text = "setup(\n    name='demo-pkg',\n    version = \"1.2\",\n)";
        let package = parse_setup_py(text).unwrap();
        assert_eq!((package.name.as_str(), package.version.as_deref()), ("demo-pkg", Some("1.2")));
    }

    #[test]
    fn cargo_workspace_members_targets_and_dependencies() {
        let root = temp_dir("cargo");
        write(&root, "Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/skip\"]\n");
        write(
            &root,
            "crates/a/Cargo.toml",
            "[package]\nname = \"a-core\"\nversion = \"0.2.0\"\n[dependencies]\nserde = \"1\"\nb = { path = \"../b\" }\nrenamed = { package = \"real\", version = \"2\", optional = true }\n[dev-dependencies]\ntempfile = \"3\"\n[[bin]]\nname = \"tool\"\npath = \"src/tool.rs\"\n",
        );
        write(&root, "crates/a/src/lib.rs", "");
        write(&root, "crates/a/src/main.rs", "");
        write(&root, "crates/a/src/tool.rs", "");
        write(&root, "crates/a/src/bin/extra.rs", "");
        write(&root, "crates/a/tests/it.rs", "");
        write(&root, "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
        write(&root, "crates/skip/Cargo.toml", "[package]\nname = \"skip\"\n");

        let manifests = discover_manifests(&root, Ecosystem::Cargo, false).unwrap();
        let by_path = |path: &str| manifests.iter().find(|manifest| manifest.path == path).unwrap();

        let workspace = by_path("Cargo.toml");
        assert_eq!(workspace.workspace.as_ref().unwrap().members, ["crates/*"]);
        assert!(workspace.package.is_none());

        let a = by_path("crates/a/Cargo.toml").package.as_ref().unwrap();
        assert_eq!((a.name.as_str(), a.version.as_deref()), ("a-core", Some("0.2.0")));
        assert_eq!(a.workspace_manifest.as_deref(), Some("Cargo.toml"));
        let targets = a
            .targets
            .iter()
            .map(|target| (target.kind, target.name.as_str(), target.root_path.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            [
                (TargetKind::Lib, "a_core", "crates/a/src/lib.rs"),
                (TargetKind::Bin, "a-core", "crates/a/src/main.rs"),
                (TargetKind::Bin, "extra", "crates/a/src/bin/extra.rs"),
                (TargetKind::Bin, "tool", "crates/a/src/tool.rs"),
                (TargetKind::Test, "it", "crates/a/tests/it.rs"),
            ]
        );
        let dependency = |name: &str| a.dependencies.iter().find(|dependency| dependency.name == name).unwrap();
        assert_eq!(dependency("b").path.as_deref(), Some("crates/b"));
        assert_eq!(dependency("real").rename.as_deref(), Some("renamed"));
        assert!(dependency("real").optional);
        assert_eq!(dependency("tempfile").kind, DependencyKind::Dev);

        assert_eq!(by_path("crates/b/Cargo.toml").package.as_ref().unwrap().workspace_manifest.as_deref(), Some("Cargo.toml"));
        assert_eq!(by_path("crates/skip/Cargo.toml").package.as_ref().unwrap().workspace_manifest, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn manifests_in_enclosing_directories_stop_at_the_repository_root() {
        let root = temp_dir("enclosing");
        write(&root, ".git/HEAD", "");
        write(&root, "Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
        write(&root, "member/Cargo.toml", "[package]\nname = \"member\"\n");
        write(&root, "member/src/lib.rs", "");

        let manifests = discover_manifests(&root.join("member/src"), Ecosystem::Cargo, false).unwrap();
        let paths = manifests.iter().map(|manifest| manifest.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, ["../../Cargo.toml", "../Cargo.toml"]);
        let member = manifests[1].package.as_ref().unwrap();
        assert_eq!(member.workspace_manifest.as_deref(), Some("../../Cargo.toml"));
        // The crate root is inside the analysis root, so it is reported relative to it.
        assert_eq!(member.targets[0].root_path, "lib.rs");

        // Without a repository boundary nothing above the root is trusted.
        fs::remove_dir_all(root.join(".git")).unwrap();
        let isolated = discover_manifests(&root.join("member/src"), Ecosystem::Cargo, false).unwrap();
        assert!(isolated.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn npm_workspaces_and_dependencies() {
        let root = temp_dir("npm");
        write(&root, "package.json", "{\"name\":\"mono\",\"private\":true,\"workspaces\":[\"packages/*\",\"!packages/old\"]}");
        write(
            &root,
            "packages/ui/package.json",
            "{\"name\":\"@acme/ui\",\"version\":\"1.0.0\",\"main\":\"./dist/index.js\",\"bin\":{\"ui\":\"bin/ui.js\"},\"dependencies\":{\"react\":\"^18\",\"@acme/core\":\"workspace:*\",\"local\":\"file:../local\",\"alias\":\"npm:real@^2\"},\"devDependencies\":{\"jest\":\"^29\"}}",
        );
        write(&root, "packages/old/package.json", "{\"name\":\"old\"}");
        write(&root, "packages/plain/package.json", "{\"type\":\"module\"}");

        let manifests = discover_manifests(&root, Ecosystem::Npm, false).unwrap();
        let ui = manifests.iter().find(|m| m.path == "packages/ui/package.json").unwrap().package.as_ref().unwrap();
        assert_eq!(ui.name, "@acme/ui");
        assert_eq!(ui.workspace_manifest.as_deref(), Some("package.json"));
        assert_eq!(ui.entry_files, ["packages/ui/dist/index.js", "packages/ui/bin/ui.js"]);
        let dependency = |name: &str| ui.dependencies.iter().find(|d| d.name == name).unwrap();
        assert!(dependency("@acme/core").workspace);
        assert_eq!(dependency("local").path.as_deref(), Some("packages/local"));
        assert_eq!(dependency("real").rename.as_deref(), Some("alias"));
        assert_eq!(dependency("jest").kind, DependencyKind::Dev);
        let old = manifests.iter().find(|m| m.path == "packages/old/package.json").unwrap();
        assert_eq!(old.package.as_ref().unwrap().workspace_manifest, None);
        assert!(manifests.iter().all(|m| m.path != "packages/plain/package.json"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn python_projects_prefer_pyproject_over_setup_files() {
        let root = temp_dir("python");
        write(
            &root,
            "pyproject.toml",
            "[project]\nname = \"demo\"\nversion = \"3.1\"\ndependencies = [\"requests>=2\", \"sibling\"]\n[project.optional-dependencies]\ncli = [\"click\"]\n[tool.uv.sources]\nsibling = { path = \"../sibling\" }\n[tool.uv.workspace]\nmembers = [\"libs/*\"]\n",
        );
        write(&root, "setup.cfg", "[metadata]\nname = ignored\n");
        write(&root, "libs/inner/pyproject.toml", "[project]\nname = \"inner\"\n");

        let manifests = discover_manifests(&root, Ecosystem::Python, false).unwrap();
        let root_manifest = manifests.iter().find(|m| m.path == "pyproject.toml").unwrap();
        let package = root_manifest.package.as_ref().unwrap();
        assert_eq!(package.name, "demo");
        assert_eq!(package.workspace_manifest.as_deref(), Some("pyproject.toml"));
        let dependency = |name: &str| package.dependencies.iter().find(|d| d.name == name).unwrap();
        assert_eq!(dependency("requests").requirement.as_deref(), Some(">=2"));
        assert_eq!(dependency("click").group.as_deref(), Some("cli"));
        assert_eq!(dependency("sibling").path.as_deref(), Some("../sibling"));
        assert!(manifests.iter().all(|m| m.path != "setup.cfg"));
        let inner = manifests.iter().find(|m| m.path == "libs/inner/pyproject.toml").unwrap();
        assert_eq!(inner.package.as_ref().unwrap().workspace_manifest.as_deref(), Some("pyproject.toml"));
        fs::remove_dir_all(&root).unwrap();
    }
}
