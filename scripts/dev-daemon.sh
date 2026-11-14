#!/usr/bin/env bash
# Run the daemon in the foreground with debug logging.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export RUST_LOG="${RUST_LOG:-debug,muzeec_cli_core=debug}"
cargo run --bin muzeec-cli-daemon -- --foreground
