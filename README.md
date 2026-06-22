# oob-tui

A terminal UI to **browse, query, and timeline** every interaction logged by the
interactsh OOB server (`/var/log/interactsh/interactions.jsonl` on `oob`).

It pulls the log over SSH (gzip-compressed on the wire — ~24 MB → ~1 MB), parses
every JSONL record, and gives you a sortable timeline, a free-text query box, a
protocol filter, a full request/response detail pane (never truncated), and a
timeline histogram.

## Configure

Connection settings live in a config file, not in the source. Copy the template
and set your ssh host:

```
cargo build --release
cp config.example.toml config.toml      # config.toml is gitignored
$EDITOR config.toml                     # set host = "your-ssh-alias"
```

`config.toml` keys (all optional except you need a `host` to fetch over ssh):

| key | default | meaning |
|-----|---------|---------|
| `host` | *(none)* | ssh host alias (an entry in `~/.ssh/config`, key auth) to pull the log from |
| `remote_log` | `/var/log/interactsh/interactions.jsonl` | path to interactsh's log on that host |
| `editor` | `$EDITOR`, then `nvim` | editor opened by the `e` key |
| `refresh_secs` | `60` | auto-refresh interval in seconds; `0` disables (refresh on demand with `r`) |

Lookup order (first found wins): `--config <path>` → `./config.toml` →
`~/.config/oob-tui/config.toml` → built-in defaults. CLI flags override the file.

## Run

```
oob-tui                 # read config.toml, ssh-fetch + decompress the log, open the TUI
oob-tui --host myalias  # override the configured ssh host
oob-tui --config p.toml # use a specific config file
oob-tui --file log.jsonl  # read a local jsonl file instead of ssh
oob-tui --cached        # reuse the last fetch (~/.cache/oob-tui/), no ssh
```

The first network fetch is cached to `~/.cache/oob-tui/interactions.jsonl`, so
`--cached` works offline.

## Keys

| key | action |
|-----|--------|
| `↑`/`↓`, `j`/`k` | move selection |
| `g` / `G` | first / last (newest) |
| `J`/`K`, `PgDn`/`PgUp` | scroll the detail pane |
| `/` | edit the text query (matches summary, IP, full-id, raw request/response) |
| `Enter` | apply query · `Esc` cancel |
| `p` | cycle protocol filter: ALL → HTTP → DNS |
| `s` | toggle **smart grouping** (collapse identical requests) |
| `e` | open the selected interaction/group in `$EDITOR` (default `nvim`) |
| `t` | toggle the timeline (histogram) view |
| `r` | re-fetch from the server |
| `?` | help · `q` quit |

## Smart grouping (`s`)

Hammered by a scanner sending the same request 100 times? Press `s`. Interactions
that differ only in time (same protocol, source, and raw request — for DNS: same
source, query-type, and sub-domain) collapse into a single row tagged `×N`, sorted
by most-recent activity. `j`/`k` now steps **group by group**, so you review each
*unique* request once instead of scrolling past 100 duplicates. The detail pane
shows the newest occurrence plus the group's time span and source IPs. The header
shows `group:ON` and the group/interaction counts. Press `s` again to ungroup.

## Open in editor (`e`)

Dumps the current selection to `$EDITOR` (defaults to `nvim`) as a self-contained
text file: metadata, the full untruncated raw request + response, and — for a
collapsed group — every occurrence's timestamp and source. The TUI suspends while
the editor is open and restores on exit. Set `EDITOR` to override (e.g.
`EDITOR="code -w"`).

## Views

- **List** — left: time / protocol / source IP / summary, **newest first** (most
  recent at the top; `g` jumps to newest, `G` to oldest). Right: the full raw
  request and response of the selected interaction.
- **Timeline** — a unicode histogram of the (filtered) interactions across their
  full time range, with totals, per-protocol counts, bucket width, and the newest
  interactions listed below.

The query and protocol filter apply to **both** views, so the timeline reflects
exactly what you've filtered to (e.g. query an attacker IP, switch to timeline,
see when they hit).

## Auto-refresh

By default the list re-fetches from the server every 60 seconds (`refresh_secs`),
so it works as a live monitor. The fetch runs on a background thread — the UI never
blocks — and your place is preserved: if you're at the top you stay pinned to the
newest interaction as new ones arrive; if you've scrolled into history, your
selection stays on the same interaction. The header shows `auto:60s` (or `auto:off`).
Press `r` to refresh immediately; set `refresh_secs = 0` to disable.

## License

MIT — see [LICENSE](LICENSE).
