//! Cargo: facts come from `cargo metadata`, not from `Cargo.lock` alone.
//! Dependency kinds (normal, build, dev) and workspace membership only exist
//! in the manifests, and scope depends on both. Runs locally at scan time;
//! `check` never calls cargo.

use super::{Adapter, Resolved};
use crate::model::{Ecosystem, Package, Scope, Source};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Cargo;

impl Adapter for Cargo {
    fn resolve(&self, root: &Path, lockfile: &Path) -> Result<Resolved> {
        let dir = root
            .join(lockfile)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let metadata = run_metadata(&dir.join("Cargo.toml"))?;
        from_metadata(&metadata, root, lockfile)
    }
}

#[derive(Debug, Deserialize)]
pub struct Metadata {
    packages: Vec<MetaPackage>,
    workspace_members: Vec<String>,
    resolve: Option<MetaResolve>,
}

#[derive(Debug, Deserialize)]
struct MetaPackage {
    id: String,
    name: String,
    version: String,
    license: Option<String>,
    source: Option<String>,
    manifest_path: PathBuf,
    targets: Vec<MetaTarget>,
}

#[derive(Debug, Deserialize)]
struct MetaTarget {
    kind: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MetaResolve {
    nodes: Vec<MetaNode>,
}

#[derive(Debug, Deserialize)]
struct MetaNode {
    id: String,
    deps: Vec<MetaDep>,
}

#[derive(Debug, Deserialize)]
struct MetaDep {
    pkg: String,
    dep_kinds: Vec<MetaDepKind>,
}

#[derive(Debug, Deserialize)]
struct MetaDepKind {
    kind: Option<String>,
}

/// `--locked` keeps cargo from rewriting `Cargo.lock` mid-scan (which would
/// change the hash being recorded). `--all-features` and no platform filter
/// judge everything any build could pull in. Falls back to `--offline` when
/// the network is unavailable but the registry cache has what it needs.
fn run_metadata(manifest: &Path) -> Result<Metadata> {
    let base = [
        "metadata",
        "--format-version",
        "1",
        "--all-features",
        "--locked",
        "--manifest-path",
    ];
    let mut last_error = String::new();
    for offline in [false, true] {
        let mut command = Command::new("cargo");
        command.args(base).arg(manifest);
        if offline {
            command.arg("--offline");
        }
        let output = command
            .output()
            .context("running cargo metadata; is cargo on PATH?")?;
        if output.status.success() {
            return serde_json::from_slice(&output.stdout).context("parsing cargo metadata output");
        }
        last_error = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    bail!(
        "cargo metadata failed for {}:\n{last_error}",
        manifest.display()
    )
}

pub fn from_metadata(metadata: &Metadata, root: &Path, lockfile: &Path) -> Result<Resolved> {
    let by_id: BTreeMap<&str, &MetaPackage> = metadata
        .packages
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let members: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let nodes: BTreeMap<&str, &MetaNode> = metadata
        .resolve
        .as_ref()
        .context("cargo metadata returned no resolve graph")?
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();

    let scopes = scopes(&members, &nodes, &by_id);

    let mut packages: Vec<Package> = scopes
        .iter()
        .filter(|(id, _)| !members.contains(*id))
        .filter_map(|(id, scope)| by_id.get(id).map(|p| (p, *scope)))
        .map(|(p, scope)| Package {
            ecosystem: Ecosystem::Cargo,
            name: p.name.clone(),
            version: p.version.clone(),
            source: source(p.source.as_deref()),
            declared: p.license.clone().filter(|l| !l.trim().is_empty()),
            scope,
        })
        .collect();
    packages.sort();
    packages.dedup();

    // Scope depends on the manifests as much as on the lockfile: moving a
    // dependency to [dev-dependencies] changes scope without touching
    // Cargo.lock. Hash every workspace manifest alongside it.
    let mut inputs = vec![lockfile.to_path_buf()];
    let lock_dir = root
        .join(lockfile)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let workspace_manifest = lock_dir.join("Cargo.toml");
    let mut manifests: BTreeSet<PathBuf> = members
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|p| p.manifest_path.clone())
        .collect();
    manifests.insert(workspace_manifest);
    for manifest in manifests {
        inputs.push(relative(root, &manifest)?);
    }
    inputs.sort();
    inputs.dedup();

    Ok(Resolved { packages, inputs })
}

/// The strongest scope each package is reached with, walking from the
/// workspace members. Normal edges keep the current scope, build edges cap it
/// at build, dev edges (only ever from members) give dev. A proc-macro crate
/// runs at compile time and is not linked, so it and everything below it is
/// at most build.
fn scopes<'a>(
    members: &BTreeSet<&'a str>,
    nodes: &BTreeMap<&'a str, &'a MetaNode>,
    by_id: &BTreeMap<&str, &MetaPackage>,
) -> BTreeMap<&'a str, Scope> {
    let is_proc_macro = |id: &str| {
        by_id.get(id).is_some_and(|p| {
            p.targets
                .iter()
                .any(|t| t.kind.iter().any(|k| k == "proc-macro"))
        })
    };

    let mut best: BTreeMap<&str, Scope> = BTreeMap::new();
    let mut queue: VecDeque<(&str, Scope)> = VecDeque::new();
    for member in members {
        best.insert(member, Scope::Shipped);
        queue.push_back((member, Scope::Shipped));
    }

    while let Some((id, scope)) = queue.pop_front() {
        if best.get(id).is_some_and(|b| *b > scope) {
            continue; // reached more strongly since this was queued
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in &node.deps {
            for kind in &dep.dep_kinds {
                let mut next = match kind.kind.as_deref() {
                    None => scope,
                    Some("build") => scope.min(Scope::Build),
                    Some("dev") => Scope::Dev,
                    Some(_) => scope,
                };
                if is_proc_macro(&dep.pkg) {
                    next = next.min(Scope::Build);
                }
                let improved = best.get(dep.pkg.as_str()).is_none_or(|b| next > *b);
                if improved {
                    best.insert(dep.pkg.as_str(), next);
                    queue.push_back((dep.pkg.as_str(), next));
                }
            }
        }
    }
    best
}

fn source(raw: Option<&str>) -> Source {
    match raw {
        None => Source::Path,
        Some(s)
            if s == "registry+https://github.com/rust-lang/crates.io-index"
                || s == "sparse+https://index.crates.io/" =>
        {
            Source::Registry
        }
        Some(s) if s.starts_with("git+") => Source::Git(s.trim_start_matches("git+").to_string()),
        Some(s) => Source::RegistryUrl(
            s.trim_start_matches("registry+")
                .trim_start_matches("sparse+")
                .to_string(),
        ),
    }
}

fn relative(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.strip_prefix(&root)
        .map(Path::to_path_buf)
        .with_context(|| format!("{} is outside the repo root", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace member `app` depending on:
    /// - `lib` (normal) → `leaf` (normal)
    /// - `derive` (proc-macro, normal) → `syn` (normal)
    /// - `cc` (build)
    /// - `test-util` (dev) → `leaf` (normal): leaf must stay shipped
    fn metadata() -> Metadata {
        let pkg = |id: &str, license: Option<&str>, proc_macro: bool| {
            serde_json::json!({
                "id": id, "name": id, "version": "1.0.0", "license": license,
                "source": if id == "app" { serde_json::Value::Null } else {
                    serde_json::json!("registry+https://github.com/rust-lang/crates.io-index") },
                "manifest_path": format!("/nowhere/{id}/Cargo.toml"),
                "targets": [{"kind": [if proc_macro { "proc-macro" } else { "lib" }]}],
            })
        };
        let dep = |pkg: &str, kind: Option<&str>| serde_json::json!({"pkg": pkg, "dep_kinds": [{"kind": kind}]});
        serde_json::from_value(serde_json::json!({
            "packages": [
                pkg("app", None, false), pkg("lib", Some("MIT"), false),
                pkg("leaf", Some("MIT/Apache-2.0"), false), pkg("derive", Some("MIT"), true),
                pkg("syn", Some("MIT"), false), pkg("cc", Some("MIT"), false),
                pkg("test-util", Some("MPL-2.0"), false),
            ],
            "workspace_members": ["app"],
            "resolve": {"nodes": [
                {"id": "app", "deps": [dep("lib", None), dep("derive", None), dep("cc", Some("build")), dep("test-util", Some("dev"))]},
                {"id": "lib", "deps": [dep("leaf", None)]},
                {"id": "derive", "deps": [dep("syn", None)]},
                {"id": "test-util", "deps": [dep("leaf", None)]},
                {"id": "leaf", "deps": []}, {"id": "syn", "deps": []}, {"id": "cc", "deps": []},
            ]},
        }))
        .unwrap()
    }

    fn scope_of(resolved: &Resolved, name: &str) -> Scope {
        resolved
            .packages
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .scope
    }

    #[test]
    fn scopes_follow_edges() {
        let resolved =
            from_metadata(&metadata(), Path::new("/nowhere"), Path::new("Cargo.lock")).unwrap();
        assert_eq!(scope_of(&resolved, "lib"), Scope::Shipped);
        assert_eq!(
            scope_of(&resolved, "leaf"),
            Scope::Shipped,
            "strongest path wins"
        );
        assert_eq!(
            scope_of(&resolved, "derive"),
            Scope::Build,
            "proc-macros are build"
        );
        assert_eq!(
            scope_of(&resolved, "syn"),
            Scope::Build,
            "below a proc-macro is build"
        );
        assert_eq!(scope_of(&resolved, "cc"), Scope::Build);
        assert_eq!(scope_of(&resolved, "test-util"), Scope::Dev);
    }

    #[test]
    fn workspace_members_are_not_judged() {
        let resolved =
            from_metadata(&metadata(), Path::new("/nowhere"), Path::new("Cargo.lock")).unwrap();
        assert!(resolved.packages.iter().all(|p| p.name != "app"));
    }

    #[test]
    fn inputs_are_relative() {
        let resolved =
            from_metadata(&metadata(), Path::new("/nowhere"), Path::new("Cargo.lock")).unwrap();
        assert!(
            resolved.inputs.iter().all(|p| p.is_relative()),
            "{:?}",
            resolved.inputs
        );
        assert!(resolved.inputs.contains(&PathBuf::from("Cargo.lock")));
        assert!(resolved.inputs.contains(&PathBuf::from("app/Cargo.toml")));
    }

    #[test]
    fn sources_carry_no_local_paths() {
        assert_eq!(source(None), Source::Path);
        assert_eq!(
            source(Some("sparse+https://index.crates.io/")),
            Source::Registry
        );
        assert_eq!(
            source(Some("git+https://github.com/x/y?rev=abc#abc")),
            Source::Git("https://github.com/x/y?rev=abc#abc".into())
        );
    }
}
