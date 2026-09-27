//! One adapter per ecosystem. An adapter turns one lockfile into package facts
//! (name, version, source, declared licence, scope) plus the files those facts
//! depend on, so `check` can tell when they are stale.
//!
//! Licence texts for notices (`sources`) join the trait in epic 06.

pub mod cargo;
pub mod pnpm;

use crate::model::{Ecosystem, Package};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct Resolved {
    pub packages: Vec<Package>,
    /// Every file the facts were derived from, relative to the repo root:
    /// the lockfile itself and, where scope depends on them, the manifests.
    pub inputs: Vec<PathBuf>,
}

pub trait Adapter {
    /// Resolve the lockfile at `lockfile` (relative to `root`).
    fn resolve(&self, root: &Path, lockfile: &Path) -> Result<Resolved>;
}

/// The lockfile name each ecosystem is detected by.
pub fn lockfile_ecosystem(file_name: &str) -> Option<Ecosystem> {
    match file_name {
        "Cargo.lock" => Some(Ecosystem::Cargo),
        "pnpm-lock.yaml" => Some(Ecosystem::Pnpm),
        "uv.lock" => Some(Ecosystem::Uv),
        _ => None,
    }
}

/// The adapter for an ecosystem, or `None` while it isn't built yet.
pub fn for_ecosystem(ecosystem: Ecosystem) -> Option<Box<dyn Adapter>> {
    match ecosystem {
        Ecosystem::Cargo => Some(Box::new(cargo::Cargo)),
        Ecosystem::Pnpm => Some(Box::new(pnpm::Pnpm)),
        Ecosystem::Uv => None,
    }
}
