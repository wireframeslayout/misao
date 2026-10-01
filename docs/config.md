# Configuration

English | [日本語](config.ja.md)

misao is configured by one optional JSON file, `misao.json`, plus a few environment variables.
Every `misao` command reads it; `misao serve` applies the daemon-related keys when it starts, so
changes to `scrollback`, `rings`, `logLevel`, and `socket` need a daemon restart. The `keys`
section is read by each CLI invocation.

Related documents: [Protocol](protocol.md), [CLI](cli.md), [Embedding (Node SDK)](embedding.md).

## File lookup

The first file that exists is used. Files are **not merged**. If none exists, defaults apply.

| Order | Source | If the file is missing |
|---|---|---|
| 1 | `--config PATH` | error |
| 2 | `$MISAO_CONFIG` (an empty value counts as unset) | error |
| 3 | `$MISAO_DIR/misao.json` (only when `$MISAO_DIR` is set) | next candidate |
| 4 | `~/.misao/misao.json` | defaults |

An explicitly given file (1 and 2) that does not exist is an error, so a typo is never silently
ignored. `$MISAO_DIR` must be an absolute path or start with `~/`. A file that exists but cannot
be read is always an error.

## Schema

```json
{
  "keys": { "prefix": "C-^", "detach": "d", "next": "n", "prev": "p", "list": "l" },
  "scrollback": 5000,
  "rings": { "rawBytes": 1048576, "linesBytes": 4194304, "events": 1000 },
  "socket": "~/.misao/misao.sock",
  "logLevel": "info"
}
```

Every key is optional.

| Key | Type | Default | Description |
|---|---|---|---|
| `keys.prefix` | string | `"C-^"` | Prefix key of `misao attach`. A control key only: `C-<char>` where `<char>` is one of `@ A-Z [ \ ] ^ _` (case-insensitive). `C-[` (ESC) is rejected. |
| `keys.detach` | string | `"d"` | Key after the prefix: leave `attach` (go to the pane list). |
| `keys.next` | string | `"n"` | Key after the prefix: next pane. |
| `keys.prev` | string | `"p"` | Key after the prefix: previous pane. |
| `keys.list` | string | `"l"` | Key after the prefix: show the pane list. |
| `scrollback` | integer >= 1 | `5000` | Scrollback lines of each pane's screen model. |
| `rings.rawBytes` | integer >= 1 | `1048576` (1 MiB) | Raw output ring per pane, for attach replay. |
| `rings.linesBytes` | integer >= 1 | `4194304` (4 MiB) | Line ring per pane, for `pane.subscribe_lines`. Counted as characters + 64 per line. |
| `rings.events` | integer >= 1 | `1000` | Number of events kept in the daemon-wide event ring. |
| `socket` | string | see [Socket location](#socket-location) | Socket path (absolute, or starting with `~/`). |
| `logLevel` | `"error"` \| `"warn"` \| `"info"` \| `"debug"` | `"info"` | Daemon log threshold (written to stderr). |

Details of `keys`:

- Action keys (`detach`, `next`, `prev`, `list`) are `C-<char>` or a single printable ASCII
  character. They must be distinct from each other and from the prefix.
- The default prefix is `Ctrl-^` (0x1E) because common full-screen programs do not react to it. In the
  Phase 0 check (vim, bash with readline, Claude Code, Codex), `Ctrl-^` changed nothing on any of
  them, while `Ctrl-]` and `Ctrl-\` changed vim's screen and `Ctrl-_` changed bash's.
- Pressing the prefix twice sends one prefix byte to the pane. Any other key after the prefix is discarded.

## Validation

| Problem | Result |
|---|---|
| Unknown key (at any level) | A warning on stderr (`unknown key "<path>" ignored`); the key is ignored and the command runs. |
| Wrong type or out-of-range value (for example `"scrollback": "5000"`, `"rings": {"events": 0}`) | Error, exit code `1`. The daemon does not start and no command runs. |
| Invalid JSON, unreadable file, or an explicit path that does not exist | Error, exit code `1`. |
| Invalid `keys` (non-control prefix, duplicates, collision with the prefix) | Error, exit code `1`. |
| Socket path too long or not absolute | Error, exit code `1`. |

When a file has both unknown keys and invalid values, the invalid values win (error).

## Socket location

The socket path is the first of:

| Order | Source |
|---|---|
| 1 | `$MISAO_SOCKET` (empty counts as unset) |
| 2 | `socket` in `misao.json` |
| 3 | `$MISAO_DIR/misao.sock` |
| 4 | `~/.misao/misao.sock` |

`misao serve --socket PATH` overrides all of these for that daemon (a relative path is resolved
against the current directory). The paths are used for the client and the daemon alike, so a
client and the daemon agree as long as they see the same environment and file. Panes started
by the daemon get `MISAO_SOCKET` and `MISAO_PANE_ID` in their environment.

Constraints:

- **Length**: at most **107 bytes** (the `sun_path` limit on Linux, 108 including the NUL).
  A longer path is an error that names the path and its length. Prefer a short directory such
  as `~/.misao`.
- **Absolute**: a value must be an absolute path or start with `~/` (expanded to the home
  directory).
- **Permissions**: the parent directory is created with mode `700`; an existing directory must
  be owned by the current user, must not be a symbolic link, and must have no group/other
  permission, otherwise the daemon refuses to start (it prints the `chmod 700` to run). The
  socket file is set to mode `600`.
- A stale socket (nothing is listening) is removed on start. If another daemon is already
  listening, `serve` fails.
- `daemon.pid` and `persistence.json` live next to the socket (the socket's directory), or in
  `--data DIR` for `misao serve`.

## Memory sizing

Each pane holds a raw ring, a line ring, and a screen model (`@xterm/headless`) with
`scrollback` lines. The daemon also keeps one event ring.

| Item | Cap | Rule of thumb |
|---|---|---|
| Raw ring (`rings.rawBytes`) | 1 MiB per pane | about the cap |
| Line ring (`rings.linesBytes`) | 4 MiB per pane | **about 5 MiB of heap per pane** once full |
| Event ring (`rings.events`) | 1000 items | small |
| Screen model (`scrollback`) | 5000 lines per pane | grows with `scrollback` and the screen width; not measured here |

The line-ring figure is a measurement (Node 24, heap growth after gc, ring filled with no
subscriber): about 5.2 MiB for lines of 60 characters (about 34,000 lines) and 4.7 MiB for empty
lines (about 66,000 lines). These numbers are an **indication only** and depend on the
environment. As a budget, multiply the per-pane figures by the number of panes you expect to
keep running, then add the screen models.

The line ring decides how long a subscriber may stall before it is overtaken and told
`gap: true` (see [Protocol](protocol.md#subscriptions-pull-model-and-back-pressure)). A 4 MiB ring
held a burst test of 1000 rounds with no gap on the measuring machine; lowering it saves memory
at the cost of earlier gaps for slow consumers.

## Running as a service

[deploy/README.md](../deploy/README.md) is the authoritative guide to running the daemon under
a systemd user unit (install, linger, cleanup, Node not on `PATH`, uninstall). In short:

- `deploy/misao.service` runs `misao serve` with `Restart=on-failure` and `RestartSec=1`.
- `KillMode=process`: on stop, systemd signals only the daemon, and the daemon closes every
  pane itself, so systemd and the daemon do not race over pane shutdown. After a crash systemd
  leaves processes in the cgroup alone, so agents do not survive a restart.
- After a restart, panes are not restored as live PTYs. Their metadata is kept and they appear as
  `stopped` (see [Protocol](protocol.md#pane-lifecycle-and-persistence)).
- Run `loginctl enable-linger "$USER"` once to keep the daemon running after you log out.
