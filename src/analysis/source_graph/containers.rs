//! Container-level structure: directories, workspaces, packages, Rust crates and Python import
//! packages, and which of them each source file belongs to.
//!
//! The plan is computed before any file is lowered because every artifact records the ids of
//! its containers. Ids are derived from paths, so they do not depend on lowering order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::ast::{
    DependencyAst, Ecosystem, ManifestAst, PackageAst, ProjectAst, TargetAst, TargetKind,
};
use crate::id_parts;
use crate::intern::Sym;
use crate::manifest::ancestor_directories;
use crate::supergraph::ids::{NodeId, Tag};
use crate::supergraph::{
    Container, ContainerDependency, ContainerKind, Ecosystem as SgEcosystem, stable_id,
};

/// The containers an artifact belongs to.
#[derive(Debug, Clone, Default)]
pub(crate) struct FileContainers {
    pub directory_id: Option<NodeId>,
    pub package_id: Option<NodeId>,
    pub crate_id: Option<NodeId>,
    pub import_package_id: Option<NodeId>,
}

#[derive(Debug, Default)]
pub(crate) struct ContainerPlan {
    pub containers: Vec<Container>,
    pub by_file: BTreeMap<String, FileContainers>,
}

pub(crate) fn ecosystem_for_language(language: &str) -> Option<Ecosystem> {
    match language {
        "python" => Some(Ecosystem::Python),
        "rust" => Some(Ecosystem::Cargo),
        "typescript" => Some(Ecosystem::Npm),
        _ => None,
    }
}

fn container_id(kind: &str, parts: &[&str]) -> NodeId {
    let mut all = vec![kind];
    all.extend_from_slice(parts);
    let parts = all.iter().map(|part| crate::supergraph::ids::IdPart::Str(part)).collect::<Vec<_>>();
    stable_id(Tag::Container, &parts)
}

pub(crate) fn plan_containers(
    project: &ProjectAst,
    language: &str,
    module_path: impl Fn(&str) -> String,
) -> ContainerPlan {
    let ecosystem = ecosystem_for_language(language);
    let mut plan = ContainerPlan::default();
    let artifact_id = |path: &str| stable_id(Tag::Artifact, id_parts![path]);
    let file_paths = project.files.iter().map(|file| file.path.as_str()).collect::<BTreeSet<_>>();

    // Directories: the root, every directory holding a file, and every manifest directory
    // inside the root, together with all of their ancestors.
    let mut directories = BTreeSet::from([".".to_string()]);
    let inside_root = |dir: &str| dir != ".." && !dir.starts_with("../");
    for path in &file_paths {
        directories.extend(ancestor_directories(path));
    }
    for manifest in project.manifests.iter().filter(|manifest| inside_root(&manifest.dir)) {
        directories.insert(manifest.dir.clone());
        directories.extend(ancestor_directories(&format!("{}/x", manifest.dir)));
    }
    let root_name = Path::new(&project.root)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let directory_id = |dir: &str| container_id("directory", &[dir]);
    for dir in &directories {
        plan.containers.push(Container {
            container_id: directory_id(dir),
            kind: ContainerKind::Directory,
            ecosystem: None,
            name: Sym::from(if dir == "." { root_name.clone() } else { last_segment(dir).to_string() }),
            path: Sym::from(dir.clone()),
            directory_id: None,
            parent_container_id: parent_directory(dir).map(|parent| directory_id(&parent)),
            ..empty_container()
        });
    }

    let Some(ecosystem) = ecosystem else {
        for path in &file_paths {
            plan.by_file.insert(
                path.to_string(),
                FileContainers { directory_id: Some(directory_id(&parent_of_file(path))), ..Default::default() },
            );
        }
        return plan;
    };

    // Workspaces and packages, one pair per manifest at most.
    let manifest_dir_id =
        |manifest: &ManifestAst| inside_root(&manifest.dir).then(|| directory_id(&manifest.dir));
    let mut packages = Vec::<(&ManifestAst, &PackageAst, NodeId)>::new();
    for manifest in &project.manifests {
        let package_name = manifest.package.as_ref().map(|package| package.name.clone());
        if let Some(workspace) = &manifest.workspace {
            plan.containers.push(Container {
                container_id: container_id("workspace", &[&manifest.path]),
                kind: ContainerKind::Workspace,
                ecosystem: Some(to_sg(ecosystem)),
                name: Sym::from(package_name.unwrap_or_else(|| dir_name(&manifest.dir, &root_name))),
                path: Sym::from(manifest.dir.clone()),
                directory_id: manifest_dir_id(manifest),
                manifest_path: Some(Sym::from(manifest.path.clone())),
                workspace_members: workspace.members.iter().cloned().map(Sym::from).collect(),
                workspace_exclude: workspace.exclude.iter().cloned().map(Sym::from).collect(),
                ..empty_container()
            });
        }
        if let Some(package) = &manifest.package {
            let id = container_id("package", &[&manifest.path]);
            plan.containers.push(Container {
                container_id: id,
                kind: ContainerKind::Package,
                ecosystem: Some(to_sg(ecosystem)),
                name: Sym::from(package.name.clone()),
                path: Sym::from(manifest.dir.clone()),
                directory_id: manifest_dir_id(manifest),
                parent_container_id: package
                    .workspace_manifest
                    .as_ref()
                    .map(|path| container_id("workspace", &[path])),
                manifest_path: Some(Sym::from(manifest.path.clone())),
                version: package.version.clone().map(Sym::from),
                entry_files: package.entry_files.iter().cloned().map(Sym::from).collect(),
                ..empty_container()
            });
            packages.push((manifest, package, id));
        }
    }
    resolve_dependencies(&mut plan.containers, &packages, ecosystem);

    // Rust crates: one per Cargo target.
    let mut crates = Vec::<CrateOwner>::new();
    for (manifest, package, package_id) in &packages {
        for target in &package.targets {
            let root_file = file_paths.contains(target.root_path.as_str()).then(|| target.root_path.as_str());
            let id = container_id("crate", &[&manifest.path, &format!("{:?}", target.kind), &target.name]);
            plan.containers.push(Container {
                container_id: id,
                kind: ContainerKind::Crate,
                ecosystem: Some(SgEcosystem::Cargo),
                name: Sym::from(target.name.clone()),
                path: Sym::from(parent_of_file(&target.root_path)),
                directory_id: root_file.map(|path| directory_id(&parent_of_file(path))),
                parent_container_id: Some(*package_id),
                manifest_path: Some(Sym::from(manifest.path.clone())),
                version: package.version.clone().map(Sym::from),
                module_path: root_file.map(|path| Sym::from(module_path(path))),
                root_artifact_id: root_file.map(artifact_id),
                target_kind: Some(target.kind),
                ..empty_container()
            });
            if root_file.is_some() {
                crates.push(CrateOwner::new(id, target));
            }
        }
    }

    // Python import packages: directories with `__init__.py`.
    let import_package_dirs = (ecosystem == Ecosystem::Python)
        .then(|| {
            file_paths
                .iter()
                .filter(|path| last_segment(path) == "__init__.py")
                .map(|path| parent_of_file(path))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let import_package_id = |dir: &str| container_id("import-package", &[dir]);
    for dir in &import_package_dirs {
        let init = if dir == "." { "__init__.py".to_string() } else { format!("{dir}/__init__.py") };
        let parent = parent_directory(dir)
            .filter(|parent| import_package_dirs.contains(parent))
            .map(|parent| import_package_id(&parent))
            .or_else(|| innermost_package(&packages, dir).map(|(_, _, id)| *id));
        plan.containers.push(Container {
            container_id: import_package_id(dir),
            kind: ContainerKind::ImportPackage,
            ecosystem: Some(SgEcosystem::Python),
            name: Sym::from(if dir == "." { root_name.clone() } else { last_segment(dir).to_string() }),
            path: Sym::from(dir.clone()),
            directory_id: Some(directory_id(dir)),
            parent_container_id: parent,
            module_path: Some(Sym::from(module_path(&init))),
            root_artifact_id: Some(artifact_id(&init)),
            ..empty_container()
        });
    }

    for path in &file_paths {
        let dir = parent_of_file(path);
        plan.by_file.insert(
            path.to_string(),
            FileContainers {
                directory_id: Some(directory_id(&dir)),
                package_id: innermost_package(&packages, path).map(|(_, _, id)| *id),
                crate_id: crates
                    .iter()
                    .filter_map(|owner| owner.claim(path).map(|specificity| (specificity, owner)))
                    .min_by_key(|(specificity, owner)| (std::cmp::Reverse(*specificity), owner.kind, owner.name.as_str()))
                    .map(|(_, owner)| owner.id),
                import_package_id: import_package_dirs.contains(&dir).then(|| import_package_id(&dir)),
            },
        );
    }
    plan.containers.sort_by(|left, right| left.container_id.cmp(&right.container_id));
    plan
}

fn empty_container() -> Container {
    Container {
        // Placeholder values; every constructor overrides the identifying fields.
        container_id: container_id("", &[]),
        kind: ContainerKind::Directory,
        ecosystem: None,
        name: Sym::new(""),
        path: Sym::new(""),
        directory_id: None,
        parent_container_id: None,
        manifest_path: None,
        version: None,
        module_path: None,
        root_artifact_id: None,
        target_kind: None,
        entry_files: Vec::new(),
        workspace_members: Vec::new(),
        workspace_exclude: Vec::new(),
        dependencies: Vec::new(),
    }
}

fn to_sg(ecosystem: Ecosystem) -> SgEcosystem {
    match ecosystem {
        Ecosystem::Cargo => SgEcosystem::Cargo,
        Ecosystem::Npm => SgEcosystem::Npm,
        Ecosystem::Python => SgEcosystem::Python,
    }
}

/// A crate target and the part of the source tree its module tree is taken to cover.
struct CrateOwner {
    id: NodeId,
    kind: TargetKind,
    name: String,
    root_path: String,
    /// Directory whose whole subtree belongs to the crate (`.` meaning everything).
    owned_dir: Option<String>,
}

impl CrateOwner {
    /// `lib.rs` and `main.rs` own their directory; any other root file owns itself and the
    /// directory named after it (`foo.rs` and `foo/`).
    fn new(id: NodeId, target: &TargetAst) -> Self {
        let root_path = target.root_path.clone();
        let file_name = last_segment(&root_path);
        let dir = parent_of_file(&root_path);
        let owned_dir = if matches!(file_name, "lib.rs" | "main.rs") {
            Some(dir)
        } else {
            let stem = file_name.trim_end_matches(".rs");
            Some(if dir == "." { stem.to_string() } else { format!("{dir}/{stem}") })
        };
        Self { id, kind: target.kind, name: target.name.clone(), root_path, owned_dir }
    }

    /// How specifically this crate claims `path`; `None` when it does not.
    fn claim(&self, path: &str) -> Option<usize> {
        if path == self.root_path {
            return Some(usize::MAX);
        }
        let dir = self.owned_dir.as_deref()?;
        if dir == "." {
            return Some(0);
        }
        path.strip_prefix(dir).filter(|rest| rest.starts_with('/')).map(|_| dir.len())
    }
}

fn resolve_dependencies(
    containers: &mut [Container],
    packages: &[(&ManifestAst, &PackageAst, NodeId)],
    ecosystem: Ecosystem,
) {
    let normalize = |name: &str| match ecosystem {
        Ecosystem::Npm => name.to_string(),
        _ => name.to_ascii_lowercase().replace(['_', '.'], "-"),
    };
    let mut by_name = BTreeMap::<String, Vec<NodeId>>::new();
    let mut by_dir = BTreeMap::<&str, Vec<NodeId>>::new();
    for (manifest, package, id) in packages {
        by_name.entry(normalize(&package.name)).or_default().push(*id);
        by_dir.entry(manifest.dir.as_str()).or_default().push(*id);
    }
    let unique = |ids: Option<&Vec<NodeId>>| ids.filter(|ids| ids.len() == 1).map(|ids| ids[0]);
    let resolve = |dependency: &DependencyAst| {
        dependency
            .path
            .as_deref()
            .and_then(|path| unique(by_dir.get(path)))
            .or_else(|| unique(by_name.get(&normalize(&dependency.name))))
    };

    for (_, package, id) in packages {
        let dependencies = package
            .dependencies
            .iter()
            .map(|dependency| ContainerDependency {
                name: Sym::from(dependency.name.clone()),
                rename: dependency.rename.clone().map(Sym::from),
                requirement: dependency.requirement.clone().map(Sym::from),
                kind: dependency.kind,
                optional: dependency.optional,
                path: dependency.path.clone().map(Sym::from),
                workspace: dependency.workspace,
                group: dependency.group.clone().map(Sym::from),
                resolved_container_id: resolve(dependency).filter(|resolved| resolved != id),
            })
            .collect();
        if let Some(container) = containers.iter_mut().find(|container| container.container_id == *id) {
            container.dependencies = dependencies;
        }
    }
}

/// The package whose directory most specifically contains `path` (a file or directory).
fn innermost_package<'a, 'm>(
    packages: &'a [(&'m ManifestAst, &'m PackageAst, NodeId)],
    path: &str,
) -> Option<&'a (&'m ManifestAst, &'m PackageAst, NodeId)> {
    packages
        .iter()
        .filter(|(manifest, _, _)| dir_contains(&manifest.dir, path))
        .max_by_key(|(manifest, _, _)| (dir_specificity(&manifest.dir), std::cmp::Reverse(manifest.path.as_str())))
}

/// Whether the manifest directory `dir` contains the root-relative `path`. Directories at or
/// above the analysis root (`.`, `..`, ...) contain everything analyzed.
fn dir_contains(dir: &str, path: &str) -> bool {
    dir == "." || dir.starts_with("..") || path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

/// Deeper directories rank higher; directories above the root rank below the root.
fn dir_specificity(dir: &str) -> i32 {
    if dir == "." {
        0
    } else if dir.starts_with("..") {
        -(dir.split('/').count() as i32)
    } else {
        dir.split('/').count() as i32
    }
}

fn parent_of_file(path: &str) -> String {
    path.rsplit_once('/').map_or_else(|| ".".to_string(), |(parent, _)| parent.to_string())
}

fn parent_directory(dir: &str) -> Option<String> {
    if dir == "." {
        None
    } else {
        Some(dir.rsplit_once('/').map_or_else(|| ".".to_string(), |(parent, _)| parent.to_string()))
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn dir_name(dir: &str, root_name: &str) -> String {
    if dir == "." { root_name.to_string() } else { last_segment(dir).to_string() }
}
