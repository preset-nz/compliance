//! `check` against a real repo's committed lock, with no cargo and no network
//! involved: only the files in tests/fixtures/shard.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shard")
}

fn check(root: &Path) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_preset-compliance"))
        .args(["licences", "check", "--root"])
        .arg(root)
        .output()
        .expect("runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn shard_passes_with_its_four_exceptions() {
    let (code, stdout) = check(&fixture());
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("522 packages judged, 4 excepted, 0 clarified, 0 violation(s) — pass"),
        "{stdout}"
    );
}

#[test]
fn editing_a_manifest_makes_the_lock_stale() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    let manifest = dir.path().join("src-tauri/Cargo.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("\n# moved a dependency\n");
    std::fs::write(&manifest, text).unwrap();

    let (code, stdout) = check(dir.path());
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("stale: src-tauri/Cargo.toml"), "{stdout}");
}

#[test]
fn dropping_an_exception_fails() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    let config = dir.path().join("preset-compliance.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    let without = text.replace("name = \"selectors\"", "name = \"not-selectors\"");
    std::fs::write(&config, without).unwrap();

    let (code, stdout) = check(dir.path());
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("selectors@0.36.1 (cargo) — MPL-2.0 — not allowed in shipped scope"),
        "{stdout}"
    );
    assert!(
        stdout.contains("warning: exception for `not-selectors` matched nothing"),
        "{stdout}"
    );
}
