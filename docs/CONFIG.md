# Configuration

Location: `$XDG_CONFIG_HOME/muzeec-cli/config.toml` (usually
`~/.config/muzeec-cli/config.toml`). All keys are optional; defaults shown.
`muzeec-cli config path` prints the path, `config edit` opens `$EDITOR`,
`config dump` prints the effective configuration.

```toml
[library]
music_dirs = ["~/Music"]
follow_symlinks = false
watch = true
scan_workers = 0            # 0 = auto (rayon default)

[playback]
device = "default"
engine_sample_rate = 48000  # "auto" follows the device default
buffer_ms = 20              # 10..100
gapless = true
crossfade_secs = 0.0        # 0..12; 0 = pure gapless
crossfade_on_album_change_only = false
replaygain = "track"        # "off" | "track" | "album"
volume = 70                 # 0..100
resume_on_start = true
idle_stop = false

[ui]
theme = "catppuccin-mocha"
album_art = "auto"          # auto|kitty|sixel|iterm2|halfblocks|off
visualizer = "bars"         # bars|osc|off
fps = 30

[integrations]
mpris = true
notifications = true
notify_art = true

[eq]
enabled = false
preamp_db = 0.0
bands = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]   # 31 Hz … 16 kHz, dB
```

Paths:

| Data | Location |
|---|---|
| Config | `$XDG_CONFIG_HOME/muzeec-cli/config.toml` |
| Database | `$XDG_DATA_HOME/muzeec-cli/library.db` |
| Art cache | `$XDG_CACHE_HOME/muzeec-cli/art/` |
| State | `$XDG_STATE_HOME/muzeec-cli/state.json` |
| Socket | `$XDG_RUNTIME_DIR/muzeec-cli/sock` |

Environment overrides: `MUZEEC_CLI_ROOT` (test isolation), `RUST_LOG`
(daemon logging), `NO_COLOR`, `EDITOR`.
