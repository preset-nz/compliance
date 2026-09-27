//! `init`, `add-ci`, `add-hooks`: wiring a repo up. Every edit is a small
//! text insertion into a file the repo already owns, so comments and layout
//! survive. Every command is idempotent: running it twice changes nothing.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The release this binary came from; generated CI steps pin to it.
pub const VERSION_TAG: &str = concat!("v", env!("CARGO_PKG_VERSION"));

const HOOK_COMMAND: &str = "preset-compliance licences check";

/// Every file `check` depends on. A staged change to any of them runs the hook.
const HOOK_GLOB: &str =
    "*{Cargo.lock,Cargo.toml,pnpm-lock.yaml,uv.lock,preset-compliance.toml,preset-compliance.lock}";

/// What a command did, one line per file, for the user to review and commit.
#[derive(Debug, Default)]
pub struct Changes(pub Vec<String>);

impl Changes {
    fn note(&mut self, line: impl Into<String>) {
        self.0.push(line.into());
    }
}

// ---------------------------------------------------------------- init

pub fn write_config(root: &Path) -> Result<PathBuf> {
    let path = root.join(crate::config::FILE_NAME);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    std::fs::write(
        &path,
        "# preset-compliance: https://github.com/preset-nz/compliance\n\
         [licences]\n\
         extends = \"permissive@1\"\n",
    )?;
    Ok(path)
}

// ---------------------------------------------------------------- add-ci

pub fn add_ci(root: &Path) -> Result<Changes> {
    let mut changes = Changes::default();

    // justfile: a `licences` recipe, called from `check`.
    let justfile = root.join("justfile");
    if justfile.exists() {
        let text = std::fs::read_to_string(&justfile)?;
        let edited = justfile_with_licences(&text);
        if edited != text {
            std::fs::write(&justfile, edited)?;
            changes.note("justfile: `licences` recipe, called from `check`");
        } else {
            changes.note("justfile: already wired");
        }
    } else {
        changes.note("justfile: none, skipped (CI gets its own workflow below)");
    }

    // Workflows: install the binary before every step that runs `just check`.
    let workflows = root.join(".github/workflows");
    let mut wired = false;
    if workflows.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&workflows)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
            .collect();
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file)?;
            let name = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .display()
                .to_string();
            if text.contains("preset-nz/compliance@") {
                changes.note(format!("{name}: already installs preset-compliance"));
                wired = true;
                continue;
            }
            if let Some(edited) = workflow_with_install(&text) {
                std::fs::write(&file, edited)?;
                changes.note(format!(
                    "{name}: installs preset-compliance before `just check`"
                ));
                wired = true;
            }
        }
    }
    if !wired {
        std::fs::create_dir_all(&workflows)?;
        let file = workflows.join("licences.yml");
        std::fs::write(&file, standalone_workflow())?;
        changes.note(".github/workflows/licences.yml: new, runs `licences check`");
    }
    Ok(changes)
}

fn justfile_with_licences(text: &str) -> String {
    let mut out = text.to_string();
    if !has_recipe(&out, "licences") {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(
            "\n# Licence check against the committed lock. Reads files only, no network.\n\
             # Re-resolve with `preset-compliance licences scan` after changing dependencies.\n\
             [group('quality')]\n\
             licences:\n    preset-compliance licences check\n",
        );
    }
    add_to_check(&out)
}

fn has_recipe(text: &str, name: &str) -> bool {
    text.lines().any(|l| recipe_name(l) == Some(name))
}

/// `check:` / `check arg="x":` → `check`. Indented lines and settings aren't recipes.
fn recipe_name(line: &str) -> Option<&str> {
    if line.starts_with([' ', '\t', '#', '[']) || line.starts_with("set ") || line.contains(":=") {
        return None;
    }
    let head = line.split(':').next()?;
    let name = head.split_whitespace().next()?;
    (line.contains(':') && !name.is_empty()).then_some(name.trim_start_matches('@'))
}

/// Append `just licences` to the body of the `check` recipe, if there is one
/// and it doesn't call it yet.
fn add_to_check(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| recipe_name(l) == Some("check")) else {
        return text.to_string();
    };
    let body: Vec<usize> = (start + 1..lines.len())
        .take_while(|i| lines[*i].starts_with([' ', '\t']))
        .collect();
    if body.iter().any(|i| lines[*i].trim() == "just licences") {
        return text.to_string();
    }
    let indent = body
        .first()
        .map(|i| &lines[*i][..lines[*i].len() - lines[*i].trim_start().len()])
        .unwrap_or("    ");
    let insert_at = body.last().map_or(start + 1, |i| i + 1);
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(insert_at, format!("{indent}just licences"));
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Insert an install step before each step whose `run:` is `just check`.
/// `None` when the workflow has no such step.
fn workflow_with_install(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start().trim_start_matches("- ").trim_start();
        if trimmed != "run: just check" {
            continue;
        }
        // The step starts at the nearest line at or above that begins `- `.
        let step = (0..=i)
            .rev()
            .find(|j| lines[*j].trim_start().starts_with("- "))?;
        let indent = &lines[step][..lines[step].len() - lines[step].trim_start().len()];
        let block = format!(
            "{indent}- name: Install preset-compliance\n{indent}  uses: preset-nz/compliance@{VERSION_TAG}\n"
        );
        inserts.push((step, block));
    }
    if inserts.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        for (at, block) in &inserts {
            if *at == i {
                out.push_str(block);
                out.push('\n');
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

fn standalone_workflow() -> String {
    format!(
        "name: Licences\n\n\
         on:\n  push:\n    branches: [main]\n  pull_request:\n    branches: [main]\n\n\
         permissions:\n  contents: read\n\n\
         jobs:\n  licences:\n    runs-on: ubuntu-latest\n    steps:\n\
         \x20     - uses: actions/checkout@v4\n\n\
         \x20     - uses: preset-nz/compliance@{VERSION_TAG}\n\
         \x20       with:\n\
         \x20         args: licences check\n"
    )
}

// ---------------------------------------------------------------- add-hooks

pub fn add_hooks(root: &Path) -> Result<Changes> {
    let mut changes = Changes::default();
    let file = root.join("lefthook.yml");
    if !file.exists() {
        std::fs::write(&file, new_lefthook())?;
        changes.note("lefthook.yml: new, runs `licences check` at pre-commit");
        changes.note("run `lefthook install` once to register the hook");
        return Ok(changes);
    }
    let text = std::fs::read_to_string(&file).context("reading lefthook.yml")?;
    if text.contains(HOOK_COMMAND) {
        changes.note("lefthook.yml: already runs `licences check`");
        return Ok(changes);
    }
    std::fs::write(&file, lefthook_with_command(&text))?;
    changes.note("lefthook.yml: `licences` command added to pre-commit");
    Ok(changes)
}

fn hook_block(indent: &str) -> String {
    format!(
        "{indent}licences:\n\
         {indent}  glob: \"{HOOK_GLOB}\"\n\
         {indent}  run: {HOOK_COMMAND}\n"
    )
}

fn new_lefthook() -> String {
    format!(
        "# `check` reads files only and runs in well under a second, so it fits\n\
         # pre-commit. `scan` never runs in a hook: it may touch the network.\n\
         pre-commit:\n  commands:\n{}",
        hook_block("    ")
    )
}

fn lefthook_with_command(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(pre_commit) = lines.iter().position(|l| l.trim_end() == "pre-commit:") else {
        // No pre-commit section: append one.
        let mut out = text.trim_end().to_string();
        out.push_str("\n\npre-commit:\n  commands:\n");
        out.push_str(&hook_block("    "));
        return out;
    };
    // The section runs until the next top-level key.
    let section_end = (pre_commit + 1..lines.len())
        .find(|i| !lines[*i].is_empty() && !lines[*i].starts_with([' ', '\t', '#']))
        .unwrap_or(lines.len());
    let commands = (pre_commit + 1..section_end).find(|i| lines[*i].trim_end() == "  commands:");

    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match commands {
        Some(at) => out.insert(at + 1, hook_block("    ").trim_end().to_string()),
        None => out.insert(
            pre_commit + 1,
            format!("  commands:\n{}", hook_block("    ").trim_end()),
        ),
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    const JUSTFILE: &str = "\
default:
    @just --list

[group('quality')]
check:
    cargo test
    cargo clippy

[group('build')]
build:
    cargo build --release
";

    #[test]
    fn justfile_gets_recipe_and_check_calls_it() {
        let edited = justfile_with_licences(JUSTFILE);
        assert!(
            edited.contains(
                "check:\n    cargo test\n    cargo clippy\n    just licences\n\n[group('build')]"
            ),
            "{edited}"
        );
        assert!(
            edited.contains("licences:\n    preset-compliance licences check\n"),
            "{edited}"
        );
        assert_eq!(justfile_with_licences(&edited), edited, "idempotent");
    }

    #[test]
    fn recipe_names_ignore_bodies_and_settings() {
        assert_eq!(recipe_name("check:"), Some("check"));
        assert_eq!(recipe_name("run target=\"desktop\":"), Some("run"));
        assert_eq!(recipe_name("    check:"), None);
        assert_eq!(recipe_name("set shell := [\"bash\", \"-uc\"]"), None);
    }

    const WORKFLOW: &str = "\
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Quality gate
        run: just check

      - run: just check
";

    #[test]
    fn workflow_installs_before_each_just_check() {
        let edited = workflow_with_install(WORKFLOW).unwrap();
        let expected = format!(
            "      - name: Install preset-compliance\n        uses: preset-nz/compliance@{VERSION_TAG}\n\n      - name: Quality gate\n        run: just check\n"
        );
        assert!(edited.contains(&expected), "{edited}");
        assert_eq!(edited.matches("uses: preset-nz/compliance@").count(), 2);
        assert!(workflow_with_install("jobs: {}\n").is_none());
    }

    #[test]
    fn lefthook_command_joins_an_existing_pre_commit() {
        let existing = "\
pre-commit:
  parallel: true
  commands:
    rustfmt:
      glob: \"*.rs\"
      run: rustfmt --check {staged_files}

pre-push:
  commands:
    check:
      run: just check
";
        let edited = lefthook_with_command(existing);
        assert!(
            edited.contains("  commands:\n    licences:\n      glob:"),
            "{edited}"
        );
        assert!(
            edited.contains("    rustfmt:\n"),
            "keeps existing commands: {edited}"
        );
        let pre_push = edited.find("pre-push:").unwrap();
        assert!(
            edited.find(HOOK_COMMAND).unwrap() < pre_push,
            "lands in pre-commit: {edited}"
        );
    }

    #[test]
    fn lefthook_without_pre_commit_gets_one() {
        let edited =
            lefthook_with_command("pre-push:\n  commands:\n    check:\n      run: just check\n");
        assert!(
            edited.contains("pre-commit:\n  commands:\n    licences:"),
            "{edited}"
        );
    }

    #[test]
    fn add_ci_writes_a_standalone_workflow_when_nothing_runs_just_check() {
        let dir = tempfile::tempdir().unwrap();
        let changes = add_ci(dir.path()).unwrap();
        let workflow =
            std::fs::read_to_string(dir.path().join(".github/workflows/licences.yml")).unwrap();
        assert!(
            workflow.contains(&format!("uses: preset-nz/compliance@{VERSION_TAG}")),
            "{workflow}"
        );
        assert!(workflow.contains("args: licences check"));
        assert!(changes.0.iter().any(|c| c.contains("justfile: none")));
    }
}
