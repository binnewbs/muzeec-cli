# Contributing

## Setup

```sh
rustup toolchain install stable
cargo build --workspace
cargo test --workspace
```

Runtime libraries on Arch/CachyOS: `alsa-lib`, `sqlite`, `dbus`, plus
`ffmpeg` if you regenerate fixtures and `base-devel` for packaging.

## Before sending a change

The build gate is exactly what `scripts/build-pkg.sh` runs:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`just ci` runs all three.

## Guidelines

- Keep the audio callback allocation-, lock-, and I/O-free.
- Every new command gets a daemon handler, a CLI surface, and a test where it
  is pure logic.
- Prefer wiring features through the daemon command router rather than letting
  frontends touch the database or engine.
- Do not install packages or mutate the user's system from tooling; the
  packaging script stops at the artifact.
- Add a line to `docs/ARCHITECTURE.md` for non-obvious decisions.

## Tests

- Unit tests live next to the code (`#[cfg(test)]`).
- Engine tests generate small WAV tones so they need no fixtures.
- `scripts/gen-fixtures.sh` produces committed-size MP3/FLAC/Opus/AAC files
  for codec coverage.
