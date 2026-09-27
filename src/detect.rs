//! Finding lockfiles in a repo.

use crate::adapters::lockfile_ecosystem;
use crate::model::Ecosystem;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Directories that hold installed or generated trees, never sources of truth.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
];

/// Every lockfile under `root`, relative to it, sorted. `exclude` holds
/// root-relative paths never descended into.
pub fn lockfiles(root: &Path, exclude: &[String]) -> Result<Vec<(PathBuf, Ecosystem)>> {
    let exclude: Vec<PathBuf> = exclude.iter().map(PathBuf::from).collect();
    let mut found = Vec::new();
    walk(root, Path::new(""), &exclude, &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(
    root: &Path,
    relative: &Path,
    exclude: &[PathBuf],
    found: &mut Vec<(PathBuf, Ecosystem)>,
) -> Result<()> {
    for entry in std::fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = relative.join(&*name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) || exclude.contains(&path) {
                continue;
            }
            walk(root, &path, exclude, found)?;
        } else if file_type.is_file()
            && let Some(ecosystem) = lockfile_ecosystem(&name)
        {
            found.push((path, ecosystem));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_lockfiles_and_skips_installed_trees() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "Cargo.lock",
            "web/pnpm-lock.yaml",
            "node_modules/x/pnpm-lock.yaml",
            "vendor/Cargo.lock",
            "sidecar/uv.lock",
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "").unwrap();
        }
        let found = lockfiles(root, &["vendor".into()]).unwrap();
        assert_eq!(
            found,
            vec![
                (PathBuf::from("Cargo.lock"), Ecosystem::Cargo),
                (PathBuf::from("sidecar/uv.lock"), Ecosystem::Uv),
                (PathBuf::from("web/pnpm-lock.yaml"), Ecosystem::Pnpm),
            ]
        );
    }
}
