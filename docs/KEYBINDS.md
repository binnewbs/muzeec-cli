# TUI keybindings

| Key | Action |
|---|---|
| `j` / `k`, `↑` / `↓` | move selection |
| `h` / `l`, `Tab` / `Shift+Tab` | switch pane focus |
| `Enter` | play selection / open artist, album, genre, playlist |
| `a` | add selection to queue |
| `A` | play selection next |
| `Space`, `p` | play/pause |
| `s` | stop |
| `n` / `N` | next / previous |
| `[` / `]` | seek −/+ 5 s |
| `+` / `-` | volume ±5 |
| `m` | mute |
| `r` | toggle shuffle |
| `R` | cycle loop off → all → one |
| `d` | remove selected queue item (Queue view) |
| `/` | search (supports `artist:`, `album:`, `genre:`, `year:`, `is:lossless`, `dur:>5m`) |
| `:` | command bar |
| `Esc` | cancel search/command |
| `Ctrl+C`, `q` | quit the TUI (daemon keeps playing) |

Command bar: `:loop one`, `:loop off`, `:shuffle on`, `:mix`, `:save NAME`,
`:scan`, `:search QUERY`, `:q`.

## CLI quick reference

```
muzeec-cli play [QUERY|PATH...] [--next|--now] [--json]
muzeec-cli pause|resume|toggle|stop|next|prev
muzeec-cli seek ±5s | seek 50% | seek 2:30
muzeec-cli volume 70 | volume +5 | volume -5 | mute
muzeec-cli status [--json|--waybar|--short]
muzeec-cli queue list|add|play-next|remove ID|move FROM TO|clear|shuffle|save NAME
muzeec-cli playlist list|show|create|add|remove|move|rename|delete|import|export|play
muzeec-cli library scan|rescan|stats|artists|albums|tracks|genres
muzeec-cli search QUERY [--limit N] [--json]
muzeec-cli mix [--seed QUERY] [--hours H] [--replace]
muzeec-cli loop off|one|all | shuffle on|off
muzeec-cli art [--path|--render|--save FILE] [--size 40x20]
muzeec-cli import FILE.m3u | export FILE.m3u
muzeec-cli doctor | daemon [--foreground] | completions SHELL | man | config path|edit|dump
```
