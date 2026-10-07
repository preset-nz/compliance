//! uv: the graph and scope come from `uv.lock` alone. The project's own
//! packages (`source` = `virtual` or `editable`) are the roots, and path
//! dependencies inside the repo are ours too; neither is judged. What the
//! roots reach through `dependencies` and `optional-dependencies` (extras) is
//! shipped; what only a dependency group (`[package.dev-dependencies]`, PEP
//! 735) reaches is dev. Group names are free-form and Oblique's gate treats
//! every installed package alike, so every group is dev: there is no `build`
//! scope for Python.
//!
//! Licences come from PyPI's per-version JSON, cached on disk like npm's, and
//! sit behind [`MetadataSource`] so tests never touch the network. Markers
//! are ignored: a Windows-only dependency is judged on every platform, as for
//! pnpm's optional builds.

use super::{Adapter, Resolved, cache_dir};
use crate::licence;
use crate::model::{Ecosystem, Package, Scope, Source};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const PYPI: &str = "https://pypi.org";
const PYPI_SIMPLE: &str = "https://pypi.org/simple";

/// Where a package's metadata comes from. The one network seam of this
/// adapter.
pub trait MetadataSource: Sync {
    /// The `info` object of `https://pypi.org/pypi/<name>/<version>/json`.
    fn info(&self, name: &str, version: &str) -> Result<serde_json::Value>;
}

pub struct Uv {
    source: Box<dyn MetadataSource + Send>,
}

impl Uv {
    /// The real adapter: PyPI over HTTPS, cached on disk.
    pub fn pypi() -> Uv {
        Uv::with_source(Box::new(Pypi {
            cache: cache_dir().join("pypi"),
        }))
    }

    pub fn with_source(source: Box<dyn MetadataSource + Send>) -> Uv {
        Uv { source }
    }
}

impl Adapter for Uv {
    fn resolve(&self, root: &Path, lockfile: &Path) -> Result<Resolved> {
        let text = std::fs::read_to_string(root.join(lockfile))
            .with_context(|| format!("reading {}", lockfile.display()))?;
        let facts = graph(&text).with_context(|| format!("parsing {}", lockfile.display()))?;
        let licences = licences(self.source.as_ref(), &facts)?;
        let packages = facts
            .into_iter()
            .map(|f| {
                let declared = licences
                    .get(&(f.name.clone(), f.version.clone()))
                    .cloned()
                    .flatten();
                Package {
                    ecosystem: Ecosystem::Uv,
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
struct Lockfile {
    version: u32,
    #[serde(default, rename = "package")]
    packages: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    version: Option<String>,
    source: LockSource,
    #[serde(default)]
    dependencies: Vec<DepRef>,
    #[serde(default, rename = "optional-dependencies")]
    optional: BTreeMap<String, Vec<DepRef>>,
    #[serde(default, rename = "dev-dependencies")]
    dev: BTreeMap<String, Vec<DepRef>>,
}

#[derive(Debug, Default, Deserialize)]
struct LockSource {
    registry: Option<String>,
    git: Option<String>,
    url: Option<String>,
    path: Option<String>,
    directory: Option<String>,
    editable: Option<String>,
    r#virtual: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DepRef {
    name: String,
    /// Present only where the lock holds several versions of `name`.
    version: Option<String>,
    #[serde(default)]
    extra: Vec<String>,
}

/// One package as the lockfile describes it, before its licence is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fact {
    name: String,
    version: String,
    source: Source,
    scope: Scope,
}

enum Kind {
    /// The project itself: where the walk starts.
    Root,
    /// A path dependency inside the repo: ours, but its edges are followed.
    Own,
    Other(Source),
}

fn kind(source: &LockSource) -> Kind {
    let inside = |p: &Option<String>| {
        p.as_deref()
            .is_some_and(|p| !p.starts_with("..") && !Path::new(p).is_absolute())
    };
    if source.r#virtual.is_some() || source.editable.is_some() {
        return Kind::Root;
    }
    if inside(&source.directory) || inside(&source.path) {
        return Kind::Own;
    }
    if source.directory.is_some() || source.path.is_some() {
        return Kind::Other(Source::Path);
    }
    if let Some(git) = &source.git {
        return Kind::Other(Source::Git(git.clone()));
    }
    if let Some(url) = &source.url {
        return Kind::Other(Source::RegistryUrl(url.clone()));
    }
    match source.registry.as_deref() {
        Some(url) if url.trim_end_matches('/') != PYPI_SIMPLE => {
            Kind::Other(Source::RegistryUrl(url.to_string()))
        }
        _ => Kind::Other(Source::Registry),
    }
}

/// Resolve the lockfile into one fact per `name@version`, with the strongest
/// scope any root reaches it with.
fn graph(text: &str) -> Result<Vec<Fact>> {
    let lock: Lockfile = toml::from_str(text)?;
    if lock.version != 1 {
        bail!("uv.lock version {} is not supported; 1 is", lock.version);
    }

    let kinds: Vec<Kind> = lock.packages.iter().map(|p| kind(&p.source)).collect();
    // A reference resolves to every entry of that name, narrowed by version.
    let resolve = |dep: &DepRef| -> Vec<usize> {
        lock.packages
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.name == dep.name
                    && dep
                        .version
                        .as_ref()
                        .is_none_or(|v| p.version.as_ref() == Some(v))
            })
            .map(|(i, _)| i)
            .collect()
    };

    let mut best: BTreeMap<usize, Scope> = BTreeMap::new();
    let mut queue: VecDeque<(usize, Scope)> = VecDeque::new();
    for (i, kind) in kinds.iter().enumerate() {
        if matches!(kind, Kind::Root) {
            queue.push_back((i, Scope::Shipped));
        }
    }
    if queue.is_empty() {
        bail!("no project package (source virtual or editable) to start from");
    }

    while let Some((i, scope)) = queue.pop_front() {
        if best.get(&i).is_some_and(|b| *b >= scope) {
            continue;
        }
        best.insert(i, scope);
        let entry = &lock.packages[i];
        let mut edges: Vec<(&DepRef, Scope)> =
            entry.dependencies.iter().map(|d| (d, scope)).collect();
        // The project's extras are published features, so shipped. Groups are
        // dev. Neither is read from anything but a root: a dependency
        // contributes only the extras it was asked for.
        if matches!(kinds[i], Kind::Root) {
            edges.extend(entry.optional.values().flatten().map(|d| (d, scope)));
            edges.extend(entry.dev.values().flatten().map(|d| (d, Scope::Dev)));
        }
        for (dep, scope) in edges {
            for target in resolve(dep) {
                queue.push_back((target, scope));
                for extra in &dep.extra {
                    for d in lock.packages[target]
                        .optional
                        .get(extra)
                        .into_iter()
                        .flatten()
                    {
                        for t in resolve(d) {
                            queue.push_back((t, scope));
                        }
                    }
                }
            }
        }
    }

    let mut facts: BTreeMap<(String, String), Fact> = BTreeMap::new();
    for (i, scope) in best {
        let Kind::Other(source) = &kinds[i] else {
            continue;
        };
        let entry = &lock.packages[i];
        let Some(version) = &entry.version else {
            continue;
        };
        let fact = facts
            .entry((entry.name.clone(), version.clone()))
            .or_insert(Fact {
                name: entry.name.clone(),
                version: version.clone(),
                source: source.clone(),
                scope,
            });
        fact.scope = fact.scope.max(scope);
    }
    Ok(facts.into_values().collect())
}

type Licences = BTreeMap<(String, String), Option<String>>;

/// Declared licences for every PyPI package. Packages from elsewhere get
/// `None` and need a `clarify` entry. Any failure fails the scan.
fn licences(source: &dyn MetadataSource, facts: &[Fact]) -> Result<Licences> {
    let todo: Vec<&Fact> = facts
        .iter()
        .filter(|f| f.source == Source::Registry)
        .collect();
    let queue = Mutex::new(todo.into_iter());
    let results: Mutex<Licences> = Mutex::new(BTreeMap::new());
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..16 {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().expect("queue").next();
                    let Some(fact) = next else { break };
                    match source.info(&fact.name, &fact.version) {
                        Ok(info) => {
                            let key = (fact.name.clone(), fact.version.clone());
                            results
                                .lock()
                                .expect("results")
                                .insert(key, declared_licence(&info));
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

    let mut failures = failures.into_inner().expect("failures");
    if !failures.is_empty() {
        failures.sort();
        bail!(
            "could not read licences for {} PyPI package(s):\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
    Ok(results.into_inner().expect("results"))
}

/// The licence a PyPI `info` object declares, in order of trust:
/// 1. `license_expression`, an SPDX expression (PEP 639);
/// 2. `License ::` classifiers, mapped to SPDX and joined with `AND`, as
///    pip-licenses reads several: every one must be allowed. A classifier with
///    no mapping stays as its trove name, which does not parse, so the
///    package needs a `clarify` entry;
/// 3. the free-text `license` field, recorded as written (long or multi-line
///    text is cut to a marked first line). It parses only if it happens to be
///    an SPDX id; otherwise it needs a `clarify` entry.
fn declared_licence(info: &serde_json::Value) -> Option<String> {
    let text = |key: &str| {
        info.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(expression) = text("license_expression") {
        return Some(expression.to_string());
    }

    let mut names: Vec<String> = info
        .get("classifiers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|c| c.strip_prefix("License ::"))
        .filter_map(|rest| rest.rsplit("::").next())
        .map(|name| name.trim().to_string())
        // "License :: OSI Approved" alone says nothing.
        .filter(|name| !name.is_empty() && name != "OSI Approved")
        .map(|name| licence::trove_to_spdx(&name).map_or(name, str::to_string))
        .collect();
    names.sort();
    names.dedup();
    if !names.is_empty() {
        return Some(names.join(" AND "));
    }

    let free = text("license").filter(|l| !l.eq_ignore_ascii_case("unknown"))?;
    let first = free.lines().next().unwrap_or(free).trim();
    Some(if free.len() > 100 || first.len() != free.len() {
        let cut: String = first.chars().take(60).collect();
        format!("{cut}…")
    } else {
        free.to_string()
    })
}

struct Pypi {
    cache: PathBuf,
}

impl MetadataSource for Pypi {
    fn info(&self, name: &str, version: &str) -> Result<serde_json::Value> {
        std::fs::create_dir_all(&self.cache)
            .with_context(|| format!("creating {}", self.cache.display()))?;
        // A published version never changes, so a cached document never
        // expires. Only the fields the verdict reads are kept.
        let file = self.cache.join(format!("{name}@{version}.json"));
        if let Ok(text) = std::fs::read_to_string(&file)
            && let Ok(info) = serde_json::from_str(&text)
        {
            return Ok(info);
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .build()
            .into();
        let url = format!("{PYPI}/pypi/{name}/{version}/json");
        let mut last_error = None;
        for _ in 0..3 {
            match agent.get(&url).call() {
                Ok(mut response) => {
                    let document: serde_json::Value =
                        serde_json::from_str(&response.body_mut().read_to_string()?)?;
                    let info = document.get("info").context("no `info` in the response")?;
                    let kept = serde_json::json!({
                        "license_expression": info.get("license_expression"),
                        "classifiers": info.get("classifiers"),
                        "license": info.get("license"),
                    });
                    std::fs::write(&file, serde_json::to_string(&kept)?)?;
                    return Ok(kept);
                }
                // A missing release won't appear on retry.
                Err(error @ ureq::Error::StatusCode(404)) => {
                    return Err(anyhow::Error::from(error).context("not on PyPI"));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .map(anyhow::Error::from)
            .unwrap_or_else(|| anyhow::anyhow!("no response")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{Why, check};
    use crate::config::Config;
    use crate::lock::{Input, Lock, hash_file};
    use serde_json::json;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uv")
    }

    /// Recorded PyPI documents: `pypi/<name>-<version>.json`.
    struct Recorded;

    impl MetadataSource for Recorded {
        fn info(&self, name: &str, version: &str) -> Result<serde_json::Value> {
            let file = fixtures().join(format!("pypi/{name}-{version}.json"));
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("no recorded document {}", file.display()))?;
            let document: serde_json::Value = serde_json::from_str(&text)?;
            Ok(document["info"].clone())
        }
    }

    struct Offline;

    impl MetadataSource for Offline {
        fn info(&self, _: &str, _: &str) -> Result<serde_json::Value> {
            bail!("connection refused")
        }
    }

    fn resolved() -> Vec<Package> {
        Uv::with_source(Box::new(Recorded))
            .resolve(&fixtures(), Path::new("uv.lock"))
            .unwrap()
            .packages
    }

    fn find<'a>(packages: &'a [Package], name: &str) -> &'a Package {
        packages
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    #[test]
    fn scope_comes_from_roots_and_groups() {
        let packages = resolved();
        assert_eq!(find(&packages, "numpy").scope, Scope::Shipped);
        assert_eq!(find(&packages, "tinyfont").scope, Scope::Shipped, "extra");
        assert_eq!(
            find(&packages, "packaging").scope,
            Scope::Shipped,
            "shipped beats dev: tinyfont needs it too"
        );
        assert_eq!(find(&packages, "pytest").scope, Scope::Dev);
        assert_eq!(find(&packages, "iniconfig").scope, Scope::Dev, "transitive");
        assert_eq!(find(&packages, "pyinstaller").scope, Scope::Dev);
        assert_eq!(find(&packages, "altgraph").scope, Scope::Dev);
    }

    #[test]
    fn own_packages_are_followed_not_judged() {
        let packages = resolved();
        assert!(packages.iter().all(|p| p.name != "demo"), "the project");
        assert!(packages.iter().all(|p| p.name != "demo-kernels"), "in-repo");
        assert_eq!(find(&packages, "kernel-helper").scope, Scope::Shipped);
        assert_eq!(
            find(&packages, "speedy").scope,
            Scope::Shipped,
            "an extra named by a dependency"
        );
        assert_eq!(
            find(&packages, "vendored").source,
            Source::Git("https://github.com/x/vendored?rev=abc#abc123".into())
        );
    }

    #[test]
    fn licence_sources_in_order_of_trust() {
        let packages = resolved();
        let declared = |name| find(&packages, name).declared.as_deref();
        assert_eq!(
            declared("numpy"),
            Some("BSD-3-Clause AND 0BSD AND MIT"),
            "PEP 639 wins over classifiers"
        );
        assert_eq!(declared("pytest"), Some("MIT"));
        assert_eq!(declared("altgraph"), Some("MIT"), "classifier mapped");
        assert_eq!(declared("pyinstaller"), Some("GPL-2.0-or-later"));
        assert_eq!(
            declared("tinyfont"),
            Some("BSD-3-Clause AND MIT"),
            "several classifiers: all must be allowed"
        );
        assert_eq!(
            declared("iniconfig"),
            Some("see LICENSE.txt…"),
            "multi-line free text is cut and marked"
        );
        assert_eq!(declared("kernel-helper"), Some("MIT"), "short free text");
        assert_eq!(declared("packaging"), None, "nothing declared");
        assert_eq!(declared("vendored"), None, "not on PyPI");
    }

    #[test]
    fn verdicts_need_clarify_for_free_text_and_missing() {
        let packages = resolved();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("uv.lock"), "x").unwrap();
        let input = Input {
            path: "uv.lock".into(),
            ecosystem: Some(Ecosystem::Uv),
            sha256: hash_file(dir.path(), Path::new("uv.lock")).unwrap(),
        };
        let lock = Lock::new(vec![], vec![input], packages);

        let plain = Config::parse("[licences]\nextends = \"permissive@1\"\n").unwrap();
        let report = check(dir.path(), &plain, &lock).unwrap();
        let why = |name: &str| {
            report
                .violations
                .iter()
                .find(|v| v.package.name == name)
                .map(|v| &v.why)
        };
        assert_eq!(why("iniconfig"), Some(&Why::Unparseable));
        assert_eq!(why("packaging"), Some(&Why::Missing));
        assert_eq!(why("pyinstaller"), Some(&Why::NotAllowed));
        assert_eq!(why("numpy"), None);
        assert_eq!(why("altgraph"), None);
        assert_eq!(why("tinyfont"), None);

        let settled = Config::parse(
            "[licences]\nextends = \"permissive@1\"\n\
             [[licences.clarify]]\nname = \"iniconfig\"\nexpression = \"MIT\"\nreason = \"LICENSE read\"\n\
             [[licences.clarify]]\nname = \"packaging\"\nexpression = \"Apache-2.0\"\nreason = \"LICENSE read\"\n\
             [[licences.clarify]]\nname = \"vendored\"\nexpression = \"MIT\"\nreason = \"LICENSE read\"\n\
             [[licences.exceptions]]\nname = \"pyinstaller\"\nreason = \"bootloader exception\"\n",
        )
        .unwrap();
        let report = check(dir.path(), &settled, &lock).unwrap();
        assert!(report.violations.is_empty(), "{:?}", report.violations);
    }

    #[test]
    fn declared_licence_forms() {
        let read = |v: serde_json::Value| declared_licence(&v);
        assert_eq!(
            read(json!({"classifiers": ["License :: OSI Approved :: Apache Software License"]})),
            Some("Apache-2.0".into())
        );
        assert_eq!(
            read(json!({"classifiers": ["License :: Other/Proprietary License"]})),
            Some("Other/Proprietary License".into()),
            "unmapped stays a trove name, which does not parse"
        );
        assert_eq!(
            read(json!({"license_expression": "", "license": "UNKNOWN"})),
            None
        );
        assert_eq!(
            read(json!({"license": "MIT\n\nCopyright"})),
            Some("MIT…".into())
        );
        assert!(licence::parse("MIT…").is_none());
    }

    #[test]
    fn network_failure_is_an_error_not_empty_licences() {
        let error = Uv::with_source(Box::new(Offline))
            .resolve(&fixtures(), Path::new("uv.lock"))
            .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("could not read licences"), "{text}");
        assert!(text.contains("numpy@2.5.0"), "{text}");
    }

    #[test]
    fn unusable_locks_are_refused() {
        assert!(graph("version = 2\n").is_err());
        assert!(graph("version = 1\n").is_err(), "no project to start from");
    }
}
