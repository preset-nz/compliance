# preset-compliance — Claude instructions

The Rust CLI that records every package a repo depends on, its licence and scope, and checks that record against one pinned policy preset. Public, MIT, on crates.io, as release binaries and as the `preset-nz/compliance` action. Every family repo gates through it (`just licences`), this one included. Plan, locked decisions and epics: `~/rhizomatic-preset/guidance/projects/compliance/README.md`. Session state: `~/rhizomatic-preset/guidance/handovers/compliance/handover.md`. Rules shared with the other tools: `../CLAUDE.md`.

## Layout

- `src/adapters/{cargo,pnpm,uv}.rs`: one adapter per package manager behind the trait in `mod.rs`. They gather the facts (graph, scope, licence) and judge nothing.
- `src/scan.rs` writes the lock, `src/lock.rs` keeps it deterministic, `src/check.rs` gives the verdicts from files alone.
- `src/notices.rs`: `notices`, the third-party licence file. `src/onboard.rs`: `init`, `add-ci`, `add-hooks`.
- `presets/permissive-N.toml`: the policies, compiled into the binary.
- `tests/fixtures/{shard,colours,uv}`: parity fixtures, real locks from the repos the tool replaced scripts in. Their verdicts are the ground truth for "same result as before".
- `action.yml`, `.github/workflows/release.yml` (builds binaries and creates the GitHub release on a `v*` tag), `install.yml` (installs that release on clean runners).

## Rules

- **`scan` runs locally and writes the committed lock; `check` reads files only.** No network in `check`, in CI or in `just licences`.
- **A released preset never changes.** A changed policy is a new `permissive@N`; a licence one repo needs goes in that repo's `[licences.allow]`.
- **Unknown licences fail closed.** `clarify` entries fix them. An agent never writes an exception: it reports the failing package, and Georg decides.
- **Parity before replacement.** An adapter change keeps the fixtures' verdicts, or the commit says why they moved.
- **MSRV is `rust-version = "1.88"`**, and CI proves it. It also steers dependency resolution, on purpose.
- **Releasing:** `just release` (knope: check, bump, changelog, rescan the lock, commit, tag; never pushes). Then push the tag and `cargo publish --locked`, and bump the `@vX.Y.Z` pin in `install.yml`. The procedure and its compliance row: `~/rhizomatic-preset/guidance/runbooks/publish-npm-package.md`. Pushing the tag and `cargo publish` are asked for first.

## Gotchas

- **SPDX ids are case-sensitive** and validated at config load: `apache-2.0` errors.
- **The cargo adapter hashes the workspace manifests as well as `Cargo.lock`**, so a version-only bump makes the lock stale. Rescan (`just scan`); knope's release does it for you.
- **Time scans with `--release`.** Debug builds do slow TLS. The npm registry serves scoped per-version documents uncached and sometimes hangs before a 502, hence the worker count, timeout and retries in the pnpm adapter.
- **Proc-macros and everything below them are build scope.**
- **Tests find fixtures through `env!("CARGO_MANIFEST_DIR")`, fixed at compile time.** After moving the checkout, the uv tests fail with "No such file or directory" until `cargo clean -p preset-compliance` (seen 2026-10-17, after the move from `packages/` to `tools/`).
