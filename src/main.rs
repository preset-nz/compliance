//! preset-compliance: records what a repo is built on, where it came from and
//! under what terms, and checks that record against one pinned policy.

mod adapters;
mod check;
mod config;
mod detect;
mod licence;
mod lock;
mod model;
mod preset;
mod scan;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "preset-compliance", version, about)]
struct Cli {
    /// The repo root: where preset-compliance.toml lives.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Dependency licences.
    #[command(subcommand)]
    Licences(Licences),
}

#[derive(Subcommand)]
enum Licences {
    /// Resolve every lockfile and write preset-compliance.lock. Runs locally;
    /// may use the network for package metadata.
    Scan,
    /// Judge the committed lock against the config and preset. Reads files
    /// only, no network: this is the one CI runs.
    Check,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<bool> {
    let config = config::Config::load(&cli.root)?;
    match cli.command {
        Command::Licences(Licences::Scan) => {
            let report = scan::scan(&cli.root, &config)?;
            for skipped in &report.lock.skipped {
                println!(
                    "skipped {} ({}: no adapter yet)",
                    skipped.path.display(),
                    skipped.ecosystem
                );
            }
            let state = if report.changed {
                "written"
            } else {
                "unchanged"
            };
            println!(
                "licences: {} packages from {} lockfile(s), {} {state}",
                report.lock.packages.len(),
                report
                    .lock
                    .inputs
                    .iter()
                    .filter(|i| i.ecosystem.is_some())
                    .count(),
                lock::FILE_NAME,
            );
            Ok(true)
        }
        Command::Licences(Licences::Check) => {
            let lock = lock::Lock::load(&cli.root)?;
            let report = check::check(&cli.root, &config, &lock)?;
            print_report(&report);
            Ok(report.passed())
        }
    }
}

fn print_report(report: &check::Report) {
    for path in &report.stale {
        println!("stale: {} changed since the last scan", path.display());
    }
    for path in &report.unrecorded {
        println!(
            "unrecorded: {} is not in {}",
            path.display(),
            lock::FILE_NAME
        );
    }
    if !report.stale.is_empty() || !report.unrecorded.is_empty() {
        println!("  run `preset-compliance licences scan` and commit the lock\n");
    }
    for path in &report.skipped {
        println!("skipped: {} (no adapter for it yet)", path.display());
    }
    for v in &report.violations {
        let p = &v.package;
        let licence = v.licence.as_deref().unwrap_or("none declared");
        let why = match v.why {
            check::Why::Missing => "no licence; add a clarify entry".to_string(),
            check::Why::Unparseable => "not SPDX; add a clarify entry".to_string(),
            check::Why::NotAllowed => format!("not allowed in {} scope", p.scope),
        };
        println!(
            "  {}@{} ({}) — {licence} — {why}",
            p.name, p.version, p.ecosystem
        );
    }
    for name in &report.unused_exceptions {
        println!("warning: exception for `{name}` matched nothing");
    }
    for name in &report.unused_clarify {
        println!("warning: clarify for `{name}` matched nothing");
    }
    let verdict = if report.passed() { "pass" } else { "FAIL" };
    println!(
        "licences: {} packages judged, {} excepted, {} clarified, {} violation(s) — {verdict}",
        report.judged,
        report.excepted,
        report.clarified,
        report.violations.len()
    );
}
