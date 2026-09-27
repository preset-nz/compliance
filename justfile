# preset-compliance — records what a repo is built on and checks it against one policy.
# Standard verbs: prep, install, run, check, build.

default:
    @just --list

[group('setup')]
prep:
    @echo "cargo: $(cargo --version 2>/dev/null || echo MISSING)"
    @echo "rustc: $(rustc --version 2>/dev/null || echo MISSING)"

[group('setup')]
install:
    cargo fetch --locked

# Pass-through to the CLI, e.g. `just run licences check --root ../shard`.
[group('dev')]
run *args:
    cargo run --quiet -- {{args}}

[group('quality')]
check:
    cargo fmt --all --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    just licences

# The tool judging its own dependencies, from the committed lock. No network.
[group('quality')]
licences:
    cargo run --quiet --locked -- licences check

# Re-resolve this repo's own dependencies into preset-compliance.lock.
[group('quality')]
scan:
    cargo run --quiet --locked -- licences scan

[group('quality')]
fmt:
    cargo fmt --all

[group('build')]
build:
    cargo build --release --locked
