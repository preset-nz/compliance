# Shard fixture

Copied from [preset-nz/shard](https://github.com/preset-nz/shard) at `11234f8` (MIT): `Cargo.lock`,
`pnpm-lock.yaml`, the workspace manifests, and the config and lock `preset-compliance licences scan`
produced there on 2026-09-27.

Against Shard's own `scripts/check-licenses.py`: the same 522 crates, and four MPL-2.0 exceptions
where the script needs five. `cssparser-macros` is a proc-macro, so it is build scope, where
`permissive@1` allows MPL-2.0. Shard's npm tree (402 shipped, 123 dev) passes with no exceptions;
Shard had no npm gate before.
