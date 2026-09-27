//! `check` against real repos' committed locks, with no package manager and no
//! network involved: only the files in tests/fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    fixture_named("shard")
}

fn fixture_named(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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
fn shard_passes_cargo_and_pnpm_with_its_four_exceptions() {
    let (code, stdout) = check(&fixture());
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("1047 packages judged, 4 excepted, 0 clarified, 0 violation(s) — pass"),
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

#[test]
fn a_colour_passes_with_sharp_excepted_and_lightningcss_in_dev() {
    let (code, stdout) = check(&fixture_named("colours"));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("666 packages judged, 14 excepted, 0 clarified, 0 violation(s) — pass"),
        "{stdout}"
    );

    let lock =
        std::fs::read_to_string(fixture_named("colours").join("preset-compliance.lock")).unwrap();
    let lightningcss = lock
        .split("[[packages]]")
        .find(|p| p.contains("name = \"lightningcss\""))
        .unwrap();
    assert!(lightningcss.contains("scope = \"dev\""), "{lightningcss}");
}

#[test]
fn a_colour_without_the_libvips_exception_fails_on_every_platform() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture_named("colours"), dir.path());
    let config = dir.path().join("preset-compliance.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("@img/sharp-libvips-*", "@img/not-libvips"),
    )
    .unwrap();

    let (code, stdout) = check(dir.path());
    assert_eq!(code, 1, "{stdout}");
    for platform in [
        "darwin-arm64",
        "linux-x64",
        "linuxmusl-arm64",
        "linux-s390x",
    ] {
        assert!(
            stdout.contains(&format!(
                "@img/sharp-libvips-{platform}@1.3.3 (pnpm) — LGPL-3.0-or-later"
            )),
            "{platform}: {stdout}"
        );
    }
}

#[test]
fn editing_pnpm_lock_makes_the_lock_stale() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture_named("colours"), dir.path());
    let lockfile = dir.path().join("pnpm-lock.yaml");
    let mut text = std::fs::read_to_string(&lockfile).unwrap();
    text.push('\n');
    std::fs::write(&lockfile, text).unwrap();

    let (code, stdout) = check(dir.path());
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("stale: pnpm-lock.yaml"), "{stdout}");
}
