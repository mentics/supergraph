use std::fs;
use std::path::{Path, PathBuf};

use supergraph::supergraph::{
    Artifact, Container, ContainerKind, NodeFact, NodeId, ProgramSupergraph, TargetKind,
};
use supergraph::{analyze_python_supergraph, analyze_rust_supergraph, analyze_typescript_supergraph};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("supergraph-containers-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn containers(graph: &ProgramSupergraph) -> Vec<&Container> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.fact {
            NodeFact::Container(container) => Some(container.as_ref()),
            _ => None,
        })
        .collect()
}

fn container<'a>(graph: &'a ProgramSupergraph, kind: ContainerKind, path: &str, name: &str) -> &'a Container {
    containers(graph)
        .into_iter()
        .find(|c| c.kind == kind && c.path.as_str() == path && c.name.as_str() == name)
        .unwrap_or_else(|| panic!("no {kind:?} {name} at {path}"))
}

fn artifact<'a>(graph: &'a ProgramSupergraph, path: &str) -> &'a Artifact {
    graph
        .nodes
        .iter()
        .find_map(|node| match &node.fact {
            NodeFact::Artifact(artifact) if artifact.path.as_str() == path => Some(artifact),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no artifact {path}"))
}

fn by_id(graph: &ProgramSupergraph, id: Option<NodeId>) -> &Container {
    let id = id.expect("container id");
    containers(graph).into_iter().find(|c| c.container_id == id).expect("container for id")
}

#[test]
fn rust_workspace_records_packages_crates_and_membership() {
    let root = scratch_dir("rust");
    write(&root, "Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    write(
        &root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"demo-core\"\nversion = \"0.3.0\"\n[dependencies]\nserde = \"1\"\n",
    );
    write(&root, "crates/core/src/lib.rs", "pub mod util;\npub fn run() {}\n");
    write(&root, "crates/core/src/util.rs", "pub fn help() {}\n");
    write(&root, "crates/core/src/main.rs", "fn main() { demo_core::run(); }\n");
    write(&root, "crates/core/src/bin/tool.rs", "fn main() {}\n");
    write(&root, "crates/core/tests/it.rs", "#[test] fn t() {}\n");
    write(
        &root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"demo-app\"\n[dependencies]\ndemo-core = { path = \"../core\" }\n",
    );
    write(&root, "crates/app/src/main.rs", "fn main() {}\n");

    let graph = analyze_rust_supergraph(&root).unwrap();

    let workspace = container(&graph, ContainerKind::Workspace, ".", &root.file_name().unwrap().to_string_lossy());
    assert_eq!(workspace.workspace_members[0].as_str(), "crates/*");

    let core = container(&graph, ContainerKind::Package, "crates/core", "demo-core");
    assert_eq!(core.version.as_ref().map(|v| v.as_str()), Some("0.3.0"));
    assert_eq!(core.parent_container_id, Some(workspace.container_id));
    let app = container(&graph, ContainerKind::Package, "crates/app", "demo-app");
    let dependency = app.dependencies.iter().find(|d| d.name.as_str() == "demo-core").unwrap();
    assert_eq!(dependency.path.as_ref().map(|p| p.as_str()), Some("crates/core"));
    assert_eq!(dependency.resolved_container_id, Some(core.container_id));
    assert!(core.dependencies.iter().any(|d| d.name.as_str() == "serde" && d.resolved_container_id.is_none()));

    let lib = container(&graph, ContainerKind::Crate, "crates/core/src", "demo_core");
    assert_eq!(lib.target_kind, Some(TargetKind::Lib));
    assert_eq!(lib.parent_container_id, Some(core.container_id));
    assert_eq!(lib.module_path.as_ref().map(|m| m.as_str()), Some("crates.core.src"));

    let crate_of = |path: &str| {
        let container = by_id(&graph, artifact(&graph, path).crate_id);
        (container.name.to_string(), container.target_kind.unwrap())
    };
    assert_eq!(crate_of("crates/core/src/lib.rs"), ("demo_core".to_string(), TargetKind::Lib));
    assert_eq!(crate_of("crates/core/src/util.rs"), ("demo_core".to_string(), TargetKind::Lib));
    assert_eq!(crate_of("crates/core/src/main.rs"), ("demo-core".to_string(), TargetKind::Bin));
    assert_eq!(crate_of("crates/core/src/bin/tool.rs"), ("tool".to_string(), TargetKind::Bin));
    assert_eq!(crate_of("crates/core/tests/it.rs"), ("it".to_string(), TargetKind::Test));
    assert_eq!(crate_of("crates/app/src/main.rs"), ("demo-app".to_string(), TargetKind::Bin));

    let util = artifact(&graph, "crates/core/src/util.rs");
    assert_eq!(by_id(&graph, util.package_id).name.as_str(), "demo-core");
    let directory = by_id(&graph, util.directory_id);
    assert_eq!((directory.kind, directory.path.as_str()), (ContainerKind::Directory, "crates/core/src"));
    let parent = by_id(&graph, directory.parent_container_id);
    assert_eq!(parent.path.as_str(), "crates/core");
    assert!(util.import_package_id.is_none());

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn python_records_projects_and_import_packages() {
    let root = scratch_dir("python");
    write(
        &root,
        "pyproject.toml",
        "[project]\nname = \"demo\"\nversion = \"1.0\"\ndependencies = [\"requests>=2\"]\n",
    );
    write(&root, "pkg/__init__.py", "");
    write(&root, "pkg/core.py", "def f():\n    return 1\n");
    write(&root, "pkg/sub/__init__.py", "");
    write(&root, "pkg/sub/leaf.py", "def g():\n    return 2\n");
    write(&root, "scripts/run.py", "print('x')\n");

    let graph = analyze_python_supergraph(&root).unwrap();

    let project = container(&graph, ContainerKind::Package, ".", "demo");
    assert_eq!(project.dependencies[0].name.as_str(), "requests");
    assert_eq!(project.dependencies[0].requirement.as_ref().map(|r| r.as_str()), Some(">=2"));

    let pkg = container(&graph, ContainerKind::ImportPackage, "pkg", "pkg");
    let sub = container(&graph, ContainerKind::ImportPackage, "pkg/sub", "sub");
    assert_eq!(pkg.parent_container_id, Some(project.container_id));
    assert_eq!(sub.parent_container_id, Some(pkg.container_id));
    assert!(sub.root_artifact_id.is_some());
    assert!(sub.module_path.as_ref().unwrap().as_str().ends_with("pkg.sub"));

    assert_eq!(artifact(&graph, "pkg/core.py").import_package_id, Some(pkg.container_id));
    assert_eq!(artifact(&graph, "pkg/sub/leaf.py").import_package_id, Some(sub.container_id));
    assert_eq!(artifact(&graph, "scripts/run.py").import_package_id, None);
    assert_eq!(artifact(&graph, "scripts/run.py").package_id, Some(project.container_id));
    assert!(artifact(&graph, "scripts/run.py").crate_id.is_none());

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn typescript_records_workspace_packages_and_dependencies() {
    let root = scratch_dir("ts");
    write(&root, "package.json", "{\"name\":\"mono\",\"private\":true,\"workspaces\":[\"packages/*\"]}");
    write(
        &root,
        "packages/ui/package.json",
        "{\"name\":\"@acme/ui\",\"version\":\"2.0.0\",\"dependencies\":{\"@acme/core\":\"workspace:*\",\"react\":\"^18\"}}",
    );
    write(&root, "packages/ui/src/button.ts", "export function button() { return 1; }\n");
    write(&root, "packages/core/package.json", "{\"name\":\"@acme/core\"}");
    write(&root, "packages/core/index.ts", "export const x = 1;\n");

    let graph = analyze_typescript_supergraph(&root).unwrap();

    let workspace = container(&graph, ContainerKind::Workspace, ".", "mono");
    let ui = container(&graph, ContainerKind::Package, "packages/ui", "@acme/ui");
    let core = container(&graph, ContainerKind::Package, "packages/core", "@acme/core");
    assert_eq!(ui.parent_container_id, Some(workspace.container_id));
    let dependency = ui.dependencies.iter().find(|d| d.name.as_str() == "@acme/core").unwrap();
    assert!(dependency.workspace);
    assert_eq!(dependency.resolved_container_id, Some(core.container_id));
    assert_eq!(artifact(&graph, "packages/ui/src/button.ts").package_id, Some(ui.container_id));
    assert_eq!(artifact(&graph, "packages/core/index.ts").package_id, Some(core.container_id));

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn container_data_survives_serialization() {
    let root = scratch_dir("roundtrip");
    write(&root, "Cargo.toml", "[package]\nname = \"rt\"\n");
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    let graph = analyze_rust_supergraph(&root).unwrap();
    let reloaded: ProgramSupergraph = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
    assert_eq!(containers(&graph), containers(&reloaded));
    assert!(containers(&reloaded).iter().any(|c| c.kind == ContainerKind::Crate));
    fs::remove_dir_all(&root).unwrap();
}
