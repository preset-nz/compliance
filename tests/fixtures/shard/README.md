# Shard fixture

Copied from [preset-nz/shard](https://github.com/preset-nz/shard) at `11234f8` (MIT): the lockfile, the
workspace manifests, and the config and lock `preset-compliance licences scan` produced there on
2026-09-27. `pnpm-lock.yaml` is left out: the lock records it as skipped, and nothing hashes it.

Against Shard's own `scripts/check-licenses.py`: the same 522 crates, and four MPL-2.0 exceptions where
the script needs five. `cssparser-macros` is a proc-macro, so it is build scope, where `permissive@1`
allows MPL-2.0.
