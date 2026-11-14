# muzeec-cli

A terminal music player built in Rust with a real audio engine, a full local
library, and first-class desktop integration. Gapless playback, equal-power
crossfade, SQLite/FTS5 search, playlists, a ratatui interface, and MPRIS2 —
packaged for Arch.

```
muzeec-cli              # open the TUI (auto-spawns the daemon)
muzeec-cli library scan # index ~/Music
muzeec-cli play "kind of blue"
muzeec-cli status --json
```

## Design

- **Pure Rust audio**: `symphonia` decodes (MP3, FLAC, Vorbis, Opus, AAC, ALAC,
  WAV, AIFF + more), `cpal` outputs, and our own mixer handles gapless
  handoffs, crossfades, ReplayGain, EQ, and a soft limiter.
- **One writer**: a daemon owns the engine and library; CLI and TUI are IPC
  clients over a Unix socket, so multiple frontends share one player.
- **Ready for a desktop**: MPRIS2 (media keys, `playerctl`, KDE/GNOME widgets),
  notifications, `.desktop` entry, icon, and a systemd user unit shipped
  disabled.

See `PLAN.md` for the full product plan, `docs/ARCHITECTURE.md` for decisions,
and `docs/KEYBINDS.md` for the TUI key map.

## Build

```sh
cargo build --release --workspace
just test        # or cargo test --workspace
```

Runtime system libraries: ALSA (`alsa-lib`), SQLite, D-Bus. On Arch/CachyOS:

```sh
sudo pacman -S --needed base-devel rust cargo alsa-lib sqlite dbus
```

## Package

```sh
./scripts/build-pkg.sh          # builds dist/muzeec-cli-<ver>-1-x86_64.pkg.tar.zst
sudo pacman -U dist/muzeec-cli-<ver>-1-x86_64.pkg.tar.zst
```

The build script runs fmt/clippy/tests, creates a clean source tarball, runs
`makepkg`, copies the artifact to `dist/`, and prints the install command. It
**never installs anything** — `sudo pacman -U` is yours to run.

Options: `--fast` (skip checks), `--native` (machine-local CPU tuning),
`--vendor` (hermetic/offline build).

## Layout

| Crate | Purpose |
|---|---|
| `crates/muzeec-cli-core` | config, XDG paths, SQLite, scanner, engine, IPC |
| `crates/muzeec-cli-tui` | ratatui interface (IPC client only) |
| `crates/muzeec-cli` | `muzeec-cli` binary: CLI, daemon auto-spawn, TUI launch |
| `crates/muzeec-cli-daemon` | `muzeec-cli-daemon`: engine + IPC + MPRIS + notifications |

## Configuration

Config lives at `$XDG_CONFIG_HOME/muzeec-cli/config.toml`, the library at
`$XDG_DATA_HOME/muzeec-cli/library.db`, and the IPC socket at
`$XDG_RUNTIME_DIR/muzeec-cli/sock`. See `docs/CONFIG.md`.

## License

MIT.
