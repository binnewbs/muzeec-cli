default:
    @just --list

# Build the workspace.
build:
    cargo build --workspace

# Run the CLI (opens the TUI, auto-spawning the daemon).
run *args:
    cargo run --bin muzeec-cli -- {{args}}

# Run the daemon in the foreground.
daemon:
    cargo run --bin muzeec-cli-daemon -- --foreground

# Fast type check.
check:
    cargo check --workspace

# Format.
fmt:
    cargo fmt --all

# Lint (warnings are errors).
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run tests.
test:
    cargo test --workspace

# Full CI gate.
ci: fmt lint test

# Generate audio fixtures (requires ffmpeg).
fixtures:
    ./scripts/gen-fixtures.sh

# Build the Arch package (artifact only; never installs).
pkg *args:
    ./scripts/build-pkg.sh {{args}}
