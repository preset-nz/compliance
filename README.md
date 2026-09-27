# preset-compliance

Records what a repo is built on, where each part came from and under what terms, and checks that
record against one pinned policy. Licences are the first check.

**Why.** Every dependency comes with terms, and breaking them is easy: a copyleft library linked
into a closed app, a notice that never made it into the build. With AI writing much of the code it is
easier still. Say "do it" and a project ends up with packages nobody chose, under licences nobody
read. preset-compliance records every package in a repo, its licence and where it came from, and
fails the build when one breaks the policy.

It is also about credit. I want attribution when people use my work, so I give it when I use
theirs. Few ideas are new; citation shows where one came from before it became something else.

I am not a lawyer. This works for me. Nothing here is legal advice, and a passing check does not
make a project compliant with anything.

---

## How it works

Two commands, split by where they run.

- **`preset-compliance licences scan`** runs on your machine. It resolves every lockfile in the
  repo, records each package's name, version, source, declared licence and scope, and writes
  `preset-compliance.lock`. It may use the network. Commit the lock.
- **`preset-compliance licences check`** runs anywhere, CI included. It reads files only: the
  lock, `preset-compliance.toml` and the preset built into the binary. It fails when a licence is
  not allowed, when a package has no usable licence, when a lockfile or manifest changed since the
  last scan, or when a lockfile appears that the lock doesn't know.

Verdicts are never stored. Editing an exception takes effect on the next `check`, without a scan.

**Scope.** Every package is judged by how it reaches your code:

| Scope | What | Example |
|---|---|---|
| `shipped` | reachable through normal dependencies | `serde` |
| `build` | build-dependencies, proc-macros and everything below them | `syn`, `cc` |
| `dev` | reachable only through dev-dependencies | `tempfile` |

A preset sets a list per scope. `permissive@1` allows permissive licences everywhere, and MPL-2.0
for build and dev tooling only.

**Package managers.**

| Package manager | Lockfile | Support |
|---|---|---|
| Cargo | `Cargo.lock` | Checked. Resolved through `cargo metadata`, because dependency kinds and workspace membership live in the manifests, not in the lockfile. |
| pnpm | `pnpm-lock.yaml` | Detected and recorded as skipped. |
| uv | `uv.lock` | Detected and recorded as skipped. |

A skipped lockfile is listed in the lock and in every `check`, so nothing goes unscanned without
saying so. A lockfile added after the last scan fails `check` until the next scan records it.

## Configuration

`preset-compliance.toml` at the repo root:

```toml
[licences]
extends = "permissive@1"        # a preset, pinned by version
exclude = ["vendor"]            # paths never searched for lockfiles

[licences.allow]                # on top of the preset, per scope
build = ["LGPL-2.1-or-later"]

[[licences.clarify]]            # a package whose declared licence is missing or wrong
name = "some-crate"
version = "^0.4"
expression = "BSD-3-Clause"
reason = "manifest declares none; LICENSE file read by hand"

[[licences.exceptions]]         # a package allowed despite its licence
name = "selectors"
reason = "MPL-2.0 via Tauri, unmodified"
date = "2026-09-12"
by = "Georg"
```

Every `clarify` and every exception needs a reason. `version` is a semver requirement and
defaults to every version. SPDX ids are case-sensitive: `Apache-2.0`, not `apache-2.0`.

## Build it

Needs Rust 1.88 or later and [just](https://github.com/casey/just).

```sh
just install
just check      # fmt, clippy, tests, and this repo's own licence check
just run licences check --root ../some-repo
```

## Licence

[MIT](LICENSE).
