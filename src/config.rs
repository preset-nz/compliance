//! `preset-compliance.toml`: one file at the repo root. Its presence makes the
//! directory a repo the tool judges.

use crate::model::{Ecosystem, Package, Scope};
use anyhow::{Context, Result};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::path::Path;

pub const FILE_NAME: &str = "preset-compliance.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub licences: Licences,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Licences {
    /// The pinned preset, e.g. `permissive@1`.
    pub extends: String,
    /// Paths (relative to the repo root) never scanned for lockfiles.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Extra licences allowed on top of the preset, per scope.
    #[serde(default)]
    pub allow: ScopedAllow,
    #[serde(default)]
    pub clarify: Vec<Clarify>,
    #[serde(default)]
    pub exceptions: Vec<Exception>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedAllow {
    #[serde(default)]
    pub shipped: Vec<String>,
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub dev: Vec<String>,
}

impl ScopedAllow {
    pub fn for_scope(&self, scope: Scope) -> &[String] {
        match scope {
            Scope::Shipped => &self.shipped,
            Scope::Build => &self.build,
            Scope::Dev => &self.dev,
        }
    }
}

/// Which packages an entry applies to.
#[derive(Debug, Deserialize)]
pub struct Selector {
    pub name: String,
    /// Restrict to one ecosystem when a name exists in several.
    pub ecosystem: Option<Ecosystem>,
    /// A semver requirement (`^0.29`, `=1.2.3`). Omitted: every version.
    pub version: Option<String>,
}

impl Selector {
    pub fn matches(&self, package: &Package) -> bool {
        if self.name != package.name {
            return false;
        }
        if self.ecosystem.is_some_and(|e| e != package.ecosystem) {
            return false;
        }
        match &self.version {
            None => true,
            Some(req) => match (VersionReq::parse(req), Version::parse(&package.version)) {
                (Ok(req), Ok(version)) => req.matches(&version),
                // A requirement that can't be evaluated never matches: fail closed.
                _ => false,
            },
        }
    }
}

/// A package whose declared licence is missing or wrong, pinned to the SPDX
/// expression a human established for it.
#[derive(Debug, Deserialize)]
pub struct Clarify {
    #[serde(flatten)]
    pub selector: Selector,
    pub expression: String,
    pub reason: String,
    #[expect(
        dead_code,
        reason = "part of the record; read by `exceptions list` (epic 05)"
    )]
    pub date: Option<String>,
    #[expect(
        dead_code,
        reason = "part of the record; read by `exceptions list` (epic 05)"
    )]
    pub by: Option<String>,
}

/// A package allowed despite its licence, with the reason on record.
#[derive(Debug, Deserialize)]
pub struct Exception {
    #[serde(flatten)]
    pub selector: Selector,
    pub reason: String,
    #[expect(
        dead_code,
        reason = "part of the record; read by `exceptions list` (epic 05)"
    )]
    pub date: Option<String>,
    #[expect(
        dead_code,
        reason = "part of the record; read by `exceptions list` (epic 05)"
    )]
    pub by: Option<String>,
}

impl Config {
    pub fn load(root: &Path) -> Result<Config> {
        let path = root.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "no {FILE_NAME} in {}. A minimal one:\n\n[licences]\nextends = \"permissive@1\"\n",
                root.display()
            )
        })?;
        Config::parse(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text)?;
        let allow = &config.licences.allow;
        for entry in allow.shipped.iter().chain(&allow.build).chain(&allow.dev) {
            crate::licence::validate_entry(entry).map_err(anyhow::Error::msg)?;
        }
        for entry in &config.licences.exceptions {
            anyhow::ensure!(
                !entry.reason.trim().is_empty(),
                "exception for `{}` has no reason",
                entry.selector.name
            );
        }
        for entry in &config.licences.clarify {
            anyhow::ensure!(
                !entry.reason.trim().is_empty(),
                "clarify for `{}` has no reason",
                entry.selector.name
            );
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn package(name: &str, version: &str) -> Package {
        Package {
            ecosystem: Ecosystem::Cargo,
            name: name.into(),
            version: version.into(),
            source: Source::Registry,
            declared: None,
            scope: Scope::Shipped,
        }
    }

    #[test]
    fn exception_needs_a_reason() {
        let text = "[licences]\nextends = \"permissive@1\"\n[[licences.exceptions]]\nname = \"x\"\nreason = \" \"\n";
        assert!(Config::parse(text).is_err());
    }

    #[test]
    fn selector_version_ranges() {
        let text = "[licences]\nextends = \"permissive@1\"\n[[licences.exceptions]]\nname = \"selectors\"\nversion = \"^0.24\"\nreason = \"MPL via Tauri\"\n";
        let config = Config::parse(text).unwrap();
        let selector = &config.licences.exceptions[0].selector;
        assert!(selector.matches(&package("selectors", "0.24.0")));
        assert!(!selector.matches(&package("selectors", "0.25.0")));
        assert!(!selector.matches(&package("cssparser", "0.24.0")));
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(Config::parse("[licences]\nextends = \"permissive@1\"\nallowed = []\n").is_err());
    }
}
