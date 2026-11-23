# ttop

htop for tmux. Shows CPU and memory per session, window, pane and process. macOS only.

```
cargo install --path .
ttop [-i SECS] [-s cpu|mem|name]
```

- CPU% is per core (can exceed 100).
- MEM is phys_footprint, same as Activity Monitor.

## Keys

| key | action |
|---|---|
| `j`/`k`, arrows | move |
| `space`/`enter` | fold |
| `h`/`l` | collapse / expand |
| `1`-`4` | show depth: sessions … processes |
| `g` | switch to selection |
| `s` | cycle sort |
| `/` | filter |
| `x` | kill (confirm with `y`) |
| `r` | refresh |
| `q` | quit |
