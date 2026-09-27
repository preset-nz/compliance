//! `licences scan`: runs locally, may use the network, writes the lock.

use crate::adapters::for_ecosystem;
use crate::config::Config;
use crate::detect;
use crate::lock::{Input, Lock, Skipped, hash_file};
use anyhow::{Result, bail};
use std::path::Path;

pub struct ScanReport {
    pub lock: Lock,
    pub changed: bool,
}

pub fn scan(root: &Path, config: &Config) -> Result<ScanReport> {
    let found = detect::lockfiles(root, &config.licences.exclude)?;
    if found.is_empty() {
        bail!("no lockfiles found under {}", root.display());
    }

    let mut skipped = Vec::new();
    let mut inputs = Vec::new();
    let mut packages = Vec::new();
    for (lockfile, ecosystem) in found {
        let Some(adapter) = for_ecosystem(ecosystem) else {
            skipped.push(Skipped {
                path: lockfile,
                ecosystem,
            });
            continue;
        };
        let resolved = adapter.resolve(root, &lockfile)?;
        for input in resolved.inputs {
            let sha256 = hash_file(root, &input)?;
            let ecosystem = (input == lockfile).then_some(ecosystem);
            inputs.push(Input {
                path: input,
                ecosystem,
                sha256,
            });
        }
        packages.extend(resolved.packages);
    }

    let lock = Lock::new(skipped, inputs, packages);
    let changed = Lock::load(root).map_or(true, |previous| previous != lock);
    if changed {
        lock.save(root)?;
    }
    Ok(ScanReport { lock, changed })
}
