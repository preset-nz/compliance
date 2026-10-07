//! `licences check`: reads files only. No network, no package managers.
//! Judges the committed lock against the config and the pinned preset.

use crate::config::Config;
use crate::detect;
use crate::licence::{self, Allowlist};
use crate::lock::{Lock, hash_file};
use crate::model::{Package, Scope};
use crate::preset::Preset;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Report {
    /// Lock inputs whose hash no longer matches, or which are gone.
    pub stale: Vec<PathBuf>,
    /// Lockfiles in the repo the lock knows nothing about.
    pub unrecorded: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
    pub violations: Vec<Violation>,
    /// Packages let through by an exception, preset ones included.
    pub excepted: usize,
    /// Of `excepted`, those let through by the preset's own exceptions.
    pub excepted_by_preset: usize,
    pub clarified: usize,
    pub judged: usize,
    pub unused_exceptions: Vec<String>,
    /// Repo exceptions the preset already covers: safe to delete.
    pub redundant_exceptions: Vec<String>,
    pub unused_clarify: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Violation {
    pub package: Package,
    /// The expression judged: the clarified one when a clarify applied.
    pub licence: Option<String>,
    pub why: Why,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Why {
    /// No licence declared and no clarify entry.
    Missing,
    /// Declared, but not valid SPDX even in lax mode.
    Unparseable,
    /// Valid, but not allowed in this package's scope.
    NotAllowed,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.stale.is_empty() && self.unrecorded.is_empty() && self.violations.is_empty()
    }
}

pub fn check(root: &Path, config: &Config, lock: &Lock) -> Result<Report> {
    let mut report = Report::default();

    // Freshness: every input still hashes the same, and no lockfile has
    // appeared that the lock doesn't account for.
    for input in &lock.inputs {
        match hash_file(root, &input.path) {
            Ok(hash) if hash == input.sha256 => {}
            _ => report.stale.push(input.path.clone()),
        }
    }
    for (path, _) in detect::lockfiles(root, &config.licences.exclude)? {
        let scanned = lock
            .inputs
            .iter()
            .any(|i| i.path == path && i.ecosystem.is_some());
        let skipped = lock.skipped.iter().any(|s| s.path == path);
        if !scanned && !skipped {
            report.unrecorded.push(path);
        }
    }
    report.skipped = lock.skipped.iter().map(|s| s.path.clone()).collect();

    // Policy.
    let preset = Preset::load(&config.licences.extends)?;
    let allow: BTreeMap<Scope, Allowlist> = Scope::ALL
        .iter()
        .map(|scope| {
            let mut list = preset.allowlist(*scope);
            list.extend(config.licences.allow.for_scope(*scope));
            // Anything allowed for shipped code is allowed for tooling too.
            if *scope != Scope::Shipped {
                list.extend(&config.licences.allow.shipped);
            }
            (*scope, list)
        })
        .collect();

    let mut exception_used = vec![false; config.licences.exceptions.len()];
    let mut exception_redundant = vec![false; config.licences.exceptions.len()];
    let mut clarify_used = vec![false; config.licences.clarify.len()];

    for package in &lock.packages {
        report.judged += 1;
        let clarified = config
            .licences
            .clarify
            .iter()
            .position(|c| c.selector.matches(package));
        let licence = match clarified {
            Some(i) => {
                clarify_used[i] = true;
                report.clarified += 1;
                Some(config.licences.clarify[i].expression.clone())
            }
            None => package.declared.clone(),
        };

        let why = match licence.as_deref().map(licence::parse) {
            None => Some(Why::Missing),
            Some(None) => Some(Why::Unparseable),
            Some(Some(expression)) => {
                (!allow[&package.scope].permits(&expression)).then_some(Why::NotAllowed)
            }
        };
        let Some(why) = why else { continue };

        // The preset's exceptions come first, and only count while the licence
        // is still the one reviewed. A repo exception covering the same
        // package is then redundant, not used.
        if preset
            .exceptions
            .iter()
            .any(|e| e.covers(package, licence.as_deref()))
        {
            report.excepted += 1;
            report.excepted_by_preset += 1;
            for (i, e) in config.licences.exceptions.iter().enumerate() {
                if e.selector.matches(package) {
                    exception_redundant[i] = true;
                }
            }
            continue;
        }
        if let Some(i) = config
            .licences
            .exceptions
            .iter()
            .position(|e| e.selector.matches(package))
        {
            exception_used[i] = true;
            report.excepted += 1;
            continue;
        }
        report.violations.push(Violation {
            package: package.clone(),
            licence,
            why,
        });
    }

    // Redundant means the preset did all the work; a repo exception that also
    // applied to some other package is simply used.
    let accounted: Vec<bool> = exception_used
        .iter()
        .zip(&exception_redundant)
        .map(|(used, redundant)| *used || *redundant)
        .collect();
    report.redundant_exceptions = config
        .licences
        .exceptions
        .iter()
        .enumerate()
        .filter(|(i, _)| exception_redundant[*i] && !exception_used[*i])
        .map(|(_, e)| e.selector.name.clone())
        .collect();
    report.unused_exceptions = unused(
        &accounted,
        config.licences.exceptions.iter().map(|e| &e.selector.name),
    );
    report.unused_clarify = unused(
        &clarify_used,
        config.licences.clarify.iter().map(|c| &c.selector.name),
    );
    Ok(report)
}

fn unused<'a>(used: &[bool], names: impl Iterator<Item = &'a String>) -> Vec<String> {
    used.iter()
        .zip(names)
        .filter(|(u, _)| !**u)
        .map(|(_, n)| n.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::Input;
    use crate::model::{Ecosystem, Source};

    fn package(name: &str, declared: Option<&str>, scope: Scope) -> Package {
        Package {
            ecosystem: Ecosystem::Cargo,
            name: name.into(),
            version: "1.0.0".into(),
            source: Source::Registry,
            declared: declared.map(Into::into),
            scope,
        }
    }

    /// A repo with a Cargo.lock whose hash the lock records.
    fn repo(packages: Vec<Package>) -> (tempfile::TempDir, Lock) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "lock").unwrap();
        let input = Input {
            path: "Cargo.lock".into(),
            ecosystem: Some(Ecosystem::Cargo),
            sha256: hash_file(dir.path(), Path::new("Cargo.lock")).unwrap(),
        };
        (dir, Lock::new(vec![], vec![input], packages))
    }

    fn config(extra: &str) -> Config {
        Config::parse(&format!("[licences]\nextends = \"permissive@1\"\n{extra}")).unwrap()
    }

    #[test]
    fn mpl_is_fine_in_build_but_not_shipped() {
        let (dir, lock) = repo(vec![
            package("cssparser-macros", Some("MPL-2.0"), Scope::Build),
            package("selectors", Some("MPL-2.0"), Scope::Shipped),
        ]);
        let report = check(dir.path(), &config(""), &lock).unwrap();
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].package.name, "selectors");
        assert_eq!(report.violations[0].why, Why::NotAllowed);
    }

    #[test]
    fn exceptions_apply_and_unused_ones_are_reported() {
        let (dir, lock) = repo(vec![package("selectors", Some("MPL-2.0"), Scope::Shipped)]);
        let config = config(
            "[[licences.exceptions]]\nname = \"selectors\"\nreason = \"via Tauri\"\n\
             [[licences.exceptions]]\nname = \"gone\"\nreason = \"no longer a dependency\"\n",
        );
        let report = check(dir.path(), &config, &lock).unwrap();
        assert!(report.passed());
        assert_eq!(report.excepted, 1);
        assert_eq!(report.unused_exceptions, vec!["gone".to_string()]);
    }

    #[test]
    fn missing_licence_fails_until_clarified() {
        let (dir, lock) = repo(vec![package("ring", None, Scope::Shipped)]);
        assert_eq!(
            check(dir.path(), &config(""), &lock).unwrap().violations[0].why,
            Why::Missing
        );
        let clarified = config(
            "[[licences.clarify]]\nname = \"ring\"\nexpression = \"MIT AND ISC\"\nreason = \"LICENSE file read by hand\"\n",
        );
        assert!(check(dir.path(), &clarified, &lock).unwrap().passed());
    }

    #[test]
    fn edited_lockfile_is_stale() {
        let (dir, lock) = repo(vec![]);
        std::fs::write(dir.path().join("Cargo.lock"), "changed").unwrap();
        let report = check(dir.path(), &config(""), &lock).unwrap();
        assert_eq!(report.stale, vec![PathBuf::from("Cargo.lock")]);
        assert!(!report.passed());
    }

    #[test]
    fn new_lockfile_is_unrecorded() {
        let (dir, lock) = repo(vec![]);
        std::fs::create_dir(dir.path().join("sidecar")).unwrap();
        std::fs::write(dir.path().join("sidecar/uv.lock"), "").unwrap();
        let report = check(dir.path(), &config(""), &lock).unwrap();
        assert_eq!(report.unrecorded, vec![PathBuf::from("sidecar/uv.lock")]);
        assert!(!report.passed());
    }

    fn config_with(preset: &str, extra: &str) -> Config {
        Config::parse(&format!("[licences]\nextends = \"{preset}\"\n{extra}")).unwrap()
    }

    fn mpl(name: &str) -> Package {
        package(name, Some("MPL-2.0"), Scope::Shipped)
    }

    #[test]
    fn preset_exception_applies_and_is_counted_apart() {
        let (dir, lock) = repo(vec![mpl("selectors"), mpl("other-mpl")]);
        let report = check(dir.path(), &config_with("permissive@2", ""), &lock).unwrap();
        assert_eq!(report.excepted, 1);
        assert_eq!(report.excepted_by_preset, 1);
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].package.name, "other-mpl");
    }

    #[test]
    fn preset_exceptions_are_not_unused_in_a_repo_without_those_crates() {
        let (dir, lock) = repo(vec![package("serde", Some("MIT"), Scope::Shipped)]);
        let report = check(dir.path(), &config_with("permissive@2", ""), &lock).unwrap();
        assert!(report.passed());
        assert_eq!(report.excepted_by_preset, 0);
        assert!(report.unused_exceptions.is_empty());
        assert!(report.redundant_exceptions.is_empty());
    }

    #[test]
    fn relicensed_package_fails_despite_the_preset_exception() {
        let (dir, lock) = repo(vec![package(
            "selectors",
            Some("GPL-3.0-only"),
            Scope::Shipped,
        )]);
        let report = check(dir.path(), &config_with("permissive@2", ""), &lock).unwrap();
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.excepted_by_preset, 0);
    }

    #[test]
    fn dual_licence_is_not_the_pinned_licence() {
        let (dir, lock) = repo(vec![package(
            "selectors",
            Some("MPL-2.0 AND GPL-3.0-only"),
            Scope::Shipped,
        )]);
        let report = check(dir.path(), &config_with("permissive@2", ""), &lock).unwrap();
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn clarified_licence_is_what_the_pin_is_matched_against() {
        let (dir, lock) = repo(vec![package("selectors", None, Scope::Shipped)]);
        let clarified = config_with(
            "permissive@2",
            "[[licences.clarify]]\nname = \"selectors\"\nexpression = \"MPL-2.0\"\nreason = \"read by hand\"\n",
        );
        let report = check(dir.path(), &clarified, &lock).unwrap();
        assert!(report.passed());
        assert_eq!(report.excepted_by_preset, 1);
    }

    #[test]
    fn repo_exception_for_a_preset_excepted_crate_is_redundant() {
        let (dir, lock) = repo(vec![mpl("selectors")]);
        let config = config_with(
            "permissive@2",
            "[[licences.exceptions]]\nname = \"selectors\"\nreason = \"via Tauri\"\n",
        );
        let report = check(dir.path(), &config, &lock).unwrap();
        assert!(report.passed());
        assert_eq!(report.excepted_by_preset, 1);
        assert_eq!(report.redundant_exceptions, vec!["selectors".to_string()]);
        assert!(report.unused_exceptions.is_empty());
    }

    #[test]
    fn repo_exception_still_covers_a_relicensed_preset_crate() {
        let (dir, lock) = repo(vec![package(
            "selectors",
            Some("GPL-3.0-only"),
            Scope::Shipped,
        )]);
        let config = config_with(
            "permissive@2",
            "[[licences.exceptions]]\nname = \"selectors\"\nreason = \"reviewed GPL\"\n",
        );
        let report = check(dir.path(), &config, &lock).unwrap();
        assert!(report.passed());
        assert_eq!(report.excepted_by_preset, 0);
        assert!(report.redundant_exceptions.is_empty());
    }

    #[test]
    fn permissive_1_ignores_preset_exceptions_entirely() {
        let (dir, lock) = repo(vec![mpl("selectors")]);
        let report = check(dir.path(), &config(""), &lock).unwrap();
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.excepted_by_preset, 0);
    }
}
