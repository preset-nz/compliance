//! Presets ship inside the binary and are pinned by version, so upgrading the
//! tool never changes a repo's verdict on its own.

use crate::config::Selector;
use crate::licence::{self, Allowlist};
use crate::model::{Package, Scope};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The preset `init` writes for a new repo.
pub const DEFAULT: &str = "permissive@2";

const PRESETS: &[(&str, &str)] = &[
    ("permissive@1", include_str!("../presets/permissive-1.toml")),
    ("permissive@2", include_str!("../presets/permissive-2.toml")),
];

#[derive(Debug, Deserialize)]
struct PresetFile {
    name: String,
    version: u32,
    scopes: BTreeMap<Scope, ScopeRules>,
    /// Added in permissive@2. Absent in @1, which never changes.
    #[serde(default)]
    exceptions: Vec<PresetException>,
}

/// A package the preset allows for everyone, pinned to the licence a human
/// reviewed. Same selector as a repo exception; the pin is the addition.
#[derive(Debug, Clone, Deserialize)]
pub struct PresetException {
    #[serde(flatten)]
    pub selector: Selector,
    /// The licence expression reviewed. The exception applies only while the
    /// package's judged licence is still this one.
    pub licence: String,
    pub reason: String,
    pub date: String,
    pub by: String,
}

impl PresetException {
    /// Selector matches and the judged licence is the one that was reviewed.
    pub fn covers(&self, package: &Package, judged: Option<&str>) -> bool {
        self.selector.matches(package)
            && judged
                .and_then(licence::parse)
                .zip(licence::parse(&self.licence))
                .is_some_and(|(have, pinned)| have.to_string() == pinned.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct ScopeRules {
    allow: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Preset {
    pub exceptions: Vec<PresetException>,
    allow: BTreeMap<Scope, Allowlist>,
}

impl Preset {
    /// Load a preset by `name@version`. An unversioned name is refused: a
    /// repo must pin the version it was judged against.
    pub fn load(id: &str) -> Result<Preset> {
        if !id.contains('@') {
            bail!("preset `{id}` needs a version, e.g. `{id}@1`");
        }
        let Some((_, text)) = PRESETS.iter().find(|(known, _)| *known == id) else {
            let known: Vec<_> = PRESETS.iter().map(|(k, _)| *k).collect();
            bail!(
                "unknown preset `{id}`; this build knows {}",
                known.join(", ")
            );
        };
        let file: PresetFile =
            toml::from_str(text).with_context(|| format!("built-in preset {id} is invalid"))?;
        debug_assert_eq!(format!("{}@{}", file.name, file.version), id);

        // Build and dev inherit everything shipped allows.
        let shipped = file
            .scopes
            .get(&Scope::Shipped)
            .map(|r| r.allow.clone())
            .unwrap_or_default();
        let allow = Scope::ALL
            .iter()
            .map(|scope| {
                let mut list = Allowlist::new(&shipped);
                if *scope != Scope::Shipped
                    && let Some(rules) = file.scopes.get(scope)
                {
                    list.extend(&rules.allow);
                }
                (*scope, list)
            })
            .collect();
        for e in &file.exceptions {
            let name = &e.selector.name;
            for (field, value) in [("reason", &e.reason), ("date", &e.date), ("by", &e.by)] {
                anyhow::ensure!(
                    !value.trim().is_empty(),
                    "preset {id}: exception for `{name}` has no {field}"
                );
            }
            anyhow::ensure!(
                licence::parse(&e.licence).is_some(),
                "preset {id}: exception for `{name}` pins `{}`, which is not SPDX",
                e.licence
            );
        }
        Ok(Preset {
            exceptions: file.exceptions,
            allow,
        })
    }

    pub fn allowlist(&self, scope: Scope) -> Allowlist {
        self.allow.get(&scope).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissive_1_loads() {
        let preset = Preset::load("permissive@1").unwrap();
        assert!(preset.allowlist(Scope::Shipped).contains("Unicode-3.0"));
        assert!(!preset.allowlist(Scope::Shipped).contains("MPL-2.0"));
        assert!(preset.allowlist(Scope::Build).contains("MPL-2.0"));
        assert!(preset.allowlist(Scope::Dev).contains("MIT"));
    }

    #[test]
    fn permissive_2_loads_with_the_four_mpl_exceptions() {
        let preset = Preset::load("permissive@2").unwrap();
        assert!(!preset.allowlist(Scope::Shipped).contains("MPL-2.0"));
        assert!(preset.allowlist(Scope::Build).contains("MPL-2.0"));
        let names: Vec<_> = preset
            .exceptions
            .iter()
            .map(|e| e.selector.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["cssparser", "dtoa-short", "selectors", "option-ext"]
        );
        assert!(preset.exceptions.iter().all(|e| e.licence == "MPL-2.0"));
    }

    #[test]
    fn permissive_2_lists_match_permissive_1() {
        let (one, two) = (
            Preset::load("permissive@1").unwrap(),
            Preset::load("permissive@2").unwrap(),
        );
        for scope in Scope::ALL {
            assert_eq!(
                format!("{:?}", one.allowlist(scope)),
                format!("{:?}", two.allowlist(scope))
            );
        }
    }

    #[test]
    fn permissive_1_has_no_exceptions() {
        assert!(Preset::load("permissive@1").unwrap().exceptions.is_empty());
    }

    #[test]
    fn every_built_in_entry_is_valid_spdx() {
        for (id, text) in PRESETS {
            let file: PresetFile = toml::from_str(text).unwrap();
            for rules in file.scopes.values() {
                for entry in &rules.allow {
                    crate::licence::validate_entry(entry).unwrap_or_else(|e| panic!("{id}: {e}"));
                }
            }
        }
    }

    #[test]
    fn unpinned_preset_is_refused() {
        assert!(Preset::load("permissive").is_err());
    }

    #[test]
    fn unknown_preset_is_refused() {
        assert!(Preset::load("permissive@99").is_err());
    }
}
