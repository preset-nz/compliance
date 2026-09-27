//! Presets ship inside the binary and are pinned by version, so upgrading the
//! tool never changes a repo's verdict on its own.

use crate::licence::Allowlist;
use crate::model::Scope;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;

const PRESETS: &[(&str, &str)] = &[("permissive@1", include_str!("../presets/permissive-1.toml"))];

#[derive(Debug, Deserialize)]
struct PresetFile {
    name: String,
    version: u32,
    scopes: BTreeMap<Scope, ScopeRules>,
}

#[derive(Debug, Deserialize)]
struct ScopeRules {
    allow: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Preset {
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
        Ok(Preset { allow })
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
