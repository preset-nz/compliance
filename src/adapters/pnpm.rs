//! pnpm: the graph and scope come from `pnpm-lock.yaml` (v9) alone. Importer
//! `dependencies` and `optionalDependencies` are shipped, `devDependencies`
//! dev. Licences come from the npm registry's per-version documents, cached on
//! disk: a published version never changes, so the cache never expires.
//!
//! Every package in the lockfile is judged, including optional platform
//! packages for platforms other than the one scanning.

use super::{Adapter, Resolved, cache_dir};
use crate::model::{Ecosystem, Package, Scope, Source};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;

pub struct Pnpm;

const REGISTRY: &str = "https://registry.npmjs.org";

impl Adapter for Pnpm {
    fn resolve(&self, root: &Path, lockfile: &Path) -> Result<Resolved> {
        let text = std::fs::read_to_string(root.join(lockfile))
            .with_context(|| format!("reading {}", lockfile.display()))?;
        let facts = graph(&text).with_context(|| format!("parsing {}", lockfile.display()))?;
        let licences = registry_licences(&facts)?;
        let packages = facts
            .into_iter()
            .map(|f| {
                let declared = licences
                    .get(&(f.name.clone(), f.version.clone()))
                    .cloned()
                    .flatten();
                Package {
                    ecosystem: Ecosystem::Pnpm,
                    name: f.name,
                    version: f.version,
                    source: f.source,
                    declared,
                    scope: f.scope,
                }
            })
            .collect();
        Ok(Resolved {
            packages,
            inputs: vec![lockfile.to_path_buf()],
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Lockfile {
    lockfile_version: String,
    #[serde(default)]
    importers: BTreeMap<String, Importer>,
    #[serde(default)]
    packages: BTreeMap<String, PackageEntry>,
    #[serde(default)]
    snapshots: BTreeMap<String, Snapshot>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Importer {
    #[serde(default)]
    dependencies: BTreeMap<String, ImporterDep>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, ImporterDep>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, ImporterDep>,
}

#[derive(Debug, Deserialize)]
struct ImporterDep {
    version: String,
}

#[derive(Debug, Default, Deserialize)]
struct PackageEntry {
    #[serde(default)]
    resolution: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, String>,
}

/// One package as the lockfile describes it, before its licence is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fact {
    name: String,
    version: String,
    source: Source,
    scope: Scope,
}

/// Resolve the lockfile into one fact per `name@version`, with the strongest
/// scope any importer reaches it with.
fn graph(text: &str) -> Result<Vec<Fact>> {
    let lock: Lockfile = serde_saphyr::from_str(text)?;
    if !lock.lockfile_version.starts_with('9') {
        bail!(
            "pnpm lockfile version {} is not supported; v9 is",
            lock.lockfile_version
        );
    }

    // Walk snapshots (peer-resolved variants) from every importer.
    let mut best: BTreeMap<String, Scope> = BTreeMap::new();
    let mut queue: VecDeque<(String, Scope)> = VecDeque::new();
    for importer in lock.importers.values() {
        let edges = [
            (&importer.dependencies, Scope::Shipped),
            (&importer.optional_dependencies, Scope::Shipped),
            (&importer.dev_dependencies, Scope::Dev),
        ];
        for (deps, scope) in edges {
            for (name, dep) in deps {
                if let Some(key) = snapshot_key(name, &dep.version) {
                    queue.push_back((key, scope));
                }
            }
        }
    }
    while let Some((key, scope)) = queue.pop_front() {
        if best.get(&key).is_some_and(|b| *b >= scope) {
            continue;
        }
        best.insert(key.clone(), scope);
        let Some(snapshot) = lock.snapshots.get(&key) else {
            continue;
        };
        for (name, version) in snapshot
            .dependencies
            .iter()
            .chain(&snapshot.optional_dependencies)
        {
            if let Some(next) = snapshot_key(name, version) {
                queue.push_back((next, scope));
            }
        }
    }

    // Collapse peer variants onto their package, keeping the strongest scope.
    let mut facts: BTreeMap<(String, String), Fact> = BTreeMap::new();
    for (key, scope) in best {
        let package_key = strip_peers(&key);
        let Some((name, version)) = split_key(package_key) else {
            continue;
        };
        let source = lock
            .packages
            .get(package_key)
            .map_or(Source::Registry, |entry| source(&entry.resolution));
        let fact = facts
            .entry((name.to_string(), version.to_string()))
            .or_insert(Fact {
                name: name.to_string(),
                version: version.to_string(),
                source,
                scope,
            });
        fact.scope = fact.scope.max(scope);
    }
    Ok(facts.into_values().collect())
}

/// The snapshot key a dependency points at, or `None` for workspace links.
/// `version` is `1.2.3`, `1.2.3(peer@4.5.6)`, or for an alias `real-name@1.2.3`.
fn snapshot_key(name: &str, version: &str) -> Option<String> {
    if version.starts_with("link:") || version.starts_with("file:") {
        return None;
    }
    let bare = strip_peers(version);
    // An alias carries its real name: `string-width-cjs: string-width@4.2.3`.
    if bare.rfind('@').is_some_and(|at| at > 0) {
        Some(version.to_string())
    } else {
        Some(format!("{name}@{version}"))
    }
}

fn strip_peers(key: &str) -> &str {
    key.split_once('(').map_or(key, |(head, _)| head)
}

/// `@scope/name@1.2.3` → (`@scope/name`, `1.2.3`).
fn split_key(key: &str) -> Option<(&str, &str)> {
    let at = key.rfind('@').filter(|at| *at > 0)?;
    Some((&key[..at], &key[at + 1..]))
}

fn source(resolution: &BTreeMap<String, String>) -> Source {
    if let Some(repo) = resolution.get("repo") {
        let commit = resolution
            .get("commit")
            .map(String::as_str)
            .unwrap_or_default();
        return Source::Git(format!("{repo}#{commit}"));
    }
    if resolution.contains_key("directory") {
        return Source::Path;
    }
    match resolution.get("tarball") {
        Some(url) if !url.starts_with(REGISTRY) => Source::RegistryUrl(url.clone()),
        _ => Source::Registry,
    }
}

type Licences = BTreeMap<(String, String), Option<String>>;

/// Declared licences for every registry package, from the cache or the npm
/// registry. Packages from elsewhere get `None` and need a `clarify` entry.
fn registry_licences(facts: &[Fact]) -> Result<Licences> {
    let cache = cache_dir().join("npm");
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;

    let todo: Vec<&Fact> = facts
        .iter()
        .filter(|f| f.source == Source::Registry)
        .collect();
    let queue = Mutex::new(todo.into_iter());
    let results: Mutex<Licences> = Mutex::new(BTreeMap::new());
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..32 {
            scope.spawn(|| {
                // One agent per worker. The registry serves scoped per-version
                // documents uncached (~0.25 s) and now and then hangs one for
                // 30 s before a 502, so: many workers, a short timeout, retries.
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(10)))
                    .build()
                    .into();
                loop {
                    let next = queue.lock().expect("queue").next();
                    let Some(fact) = next else { break };
                    match licence_for(&agent, &cache, &fact.name, &fact.version) {
                        Ok(licence) => {
                            let key = (fact.name.clone(), fact.version.clone());
                            results.lock().expect("results").insert(key, licence);
                        }
                        Err(error) => failures
                            .lock()
                            .expect("failures")
                            .push(format!("{}@{}: {error:#}", fact.name, fact.version)),
                    }
                }
            });
        }
    });

    let failures = failures.into_inner().expect("failures");
    if !failures.is_empty() {
        bail!(
            "could not read licences for {} npm package(s):\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
    Ok(results.into_inner().expect("results"))
}

#[derive(serde::Serialize, Deserialize)]
struct Cached {
    license: Option<String>,
}

fn licence_for(
    agent: &ureq::Agent,
    cache: &Path,
    name: &str,
    version: &str,
) -> Result<Option<String>> {
    let file = cache.join(format!("{}@{version}.json", name.replace('/', "%2f")));
    if let Ok(text) = std::fs::read_to_string(&file)
        && let Ok(cached) = serde_json::from_str::<Cached>(&text)
    {
        return Ok(cached.license);
    }
    let url = format!("{REGISTRY}/{name}/{version}");
    let mut last_error = None;
    for _ in 0..3 {
        match agent.get(&url).call() {
            Ok(mut response) => {
                let document: serde_json::Value =
                    serde_json::from_str(&response.body_mut().read_to_string()?)?;
                let license = declared_licence(&document);
                std::fs::write(
                    &file,
                    serde_json::to_string(&Cached {
                        license: license.clone(),
                    })?,
                )?;
                return Ok(license);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("no response")))
}

/// npm's `license` is a string (SPDX, ideally), a legacy `{type}` object, or
/// a legacy `licenses` array, read as a choice between them.
fn declared_licence(document: &serde_json::Value) -> Option<String> {
    let as_text = |v: &serde_json::Value| {
        v.as_str()
            .or_else(|| v.get("type").and_then(serde_json::Value::as_str))
            .map(str::to_string)
    };
    if let Some(licence) = document.get("license").and_then(as_text) {
        return Some(licence).filter(|l| !l.trim().is_empty());
    }
    let list: Vec<String> = document
        .get("licenses")?
        .as_array()?
        .iter()
        .filter_map(as_text)
        .collect();
    match list.len() {
        0 => None,
        1 => list.into_iter().next(),
        _ => Some(format!("({})", list.join(" OR "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
lockfileVersion: '9.0'

importers:
  .:
    dependencies:
      react:
        specifier: ^19
        version: 19.3.0
      facets:
        specifier: ^0.1
        version: 0.1.3(react@19.3.0)
      local-lib:
        specifier: workspace:*
        version: link:packages/local-lib
    devDependencies:
      tailwindcss:
        specifier: ^4
        version: 4.3.3
      string-width-cjs:
        specifier: npm:string-width@^4
        version: string-width@4.2.3

packages:
  react@19.3.0:
    resolution: {integrity: sha512-x}
  facets@0.1.3:
    resolution: {integrity: sha512-x}
  tailwindcss@4.3.3:
    resolution: {integrity: sha512-x}
  lightningcss@1.32.0:
    resolution: {integrity: sha512-x}
  lightningcss-darwin-arm64@1.32.0:
    resolution: {integrity: sha512-x}
  string-width@4.2.3:
    resolution: {integrity: sha512-x}
  '@scope/from-git@1.0.0':
    resolution: {type: git, repo: https://github.com/x/y, commit: abc123}

snapshots:
  react@19.3.0: {}
  facets@0.1.3(react@19.3.0):
    dependencies:
      react: 19.3.0
      '@scope/from-git': 1.0.0
  tailwindcss@4.3.3:
    dependencies:
      lightningcss: 1.32.0
      react: 19.3.0
  lightningcss@1.32.0:
    optionalDependencies:
      lightningcss-darwin-arm64: 1.32.0
  lightningcss-darwin-arm64@1.32.0: {}
  string-width@4.2.3: {}
  '@scope/from-git@1.0.0': {}
"#;

    fn fact<'a>(facts: &'a [Fact], name: &str) -> &'a Fact {
        facts
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    #[test]
    fn importer_sections_set_scope_and_strongest_wins() {
        let facts = graph(LOCK).unwrap();
        assert_eq!(
            fact(&facts, "react").scope,
            Scope::Shipped,
            "shipped beats dev"
        );
        assert_eq!(fact(&facts, "facets").scope, Scope::Shipped);
        assert_eq!(fact(&facts, "tailwindcss").scope, Scope::Dev);
        assert_eq!(fact(&facts, "lightningcss").scope, Scope::Dev);
    }

    #[test]
    fn optional_platform_packages_are_judged() {
        let facts = graph(LOCK).unwrap();
        assert_eq!(fact(&facts, "lightningcss-darwin-arm64").scope, Scope::Dev);
    }

    #[test]
    fn peers_aliases_and_links() {
        let facts = graph(LOCK).unwrap();
        assert_eq!(
            facts.iter().filter(|f| f.name == "facets").count(),
            1,
            "peer variants collapse"
        );
        assert_eq!(
            fact(&facts, "string-width").version,
            "4.2.3",
            "alias resolves to real name"
        );
        assert!(
            facts.iter().all(|f| f.name != "local-lib"),
            "workspace links are ours"
        );
        assert_eq!(
            fact(&facts, "@scope/from-git").source,
            Source::Git("https://github.com/x/y#abc123".into())
        );
    }

    #[test]
    fn declared_licence_forms() {
        let read = |json: &str| declared_licence(&serde_json::from_str(json).unwrap());
        assert_eq!(read(r#"{"license": "MIT"}"#), Some("MIT".into()));
        assert_eq!(read(r#"{"license": {"type": "ISC"}}"#), Some("ISC".into()));
        assert_eq!(
            read(r#"{"licenses": [{"type": "MIT"}, {"type": "Apache-2.0"}]}"#),
            Some("(MIT OR Apache-2.0)".into())
        );
        assert_eq!(read(r#"{"name": "x"}"#), None);
    }

    #[test]
    fn scoped_keys_split_on_the_last_at() {
        assert_eq!(
            split_key("@img/sharp-libvips-darwin-arm64@1.3.3"),
            Some(("@img/sharp-libvips-darwin-arm64", "1.3.3"))
        );
        assert_eq!(split_key("react@19.3.0"), Some(("react", "19.3.0")));
    }
}
