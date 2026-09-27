# A Colour fixture

Copied from [preset-nz/a-colour](https://github.com/preset-nz/a-colour) at `cd701a49` (MIT):
`pnpm-lock.yaml`, and the config and lock `preset-compliance licences scan` produced there on
2026-09-27.

Against A Colour's own `scripts/check-licenses.ts` (`pnpm licenses list`): all 478 packages it sees
are here with the same declared licences. The other 188 are optional builds for other platforms,
which an install on one machine never sees. `lightningcss` is dev scope, where `permissive@1`
allows MPL-2.0, so its exception is gone. The sharp builds that bundle libvips (LGPL-3.0) stay
exceptions: 14 packages, three entries.
