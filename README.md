# preset-compliance

Records what a repo is built on, where each part came from and under what terms, and checks that
record against one pinned policy. Licences are the first check.

**Why.** AI builds most of what I make, and what worries me most is losing track of what a project
uses and where it came from. It is too easy to say "do it" and end up with other people's work
compiled in and nobody credited. I want attribution when people use my work, so I give it when I
use theirs. Few ideas are new; citation shows where one came from before it became something else.
preset-compliance does that for dependencies: every package, its licence and its origin, written
down and checked.

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

**Ecosystems.** Cargo, through `cargo metadata`, because dependency kinds and workspace membership
live in the manifests, not in `Cargo.lock`. `pnpm-lock.yaml` and `uv.lock` are detected and
recorded as skipped.

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
