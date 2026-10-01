# CLI

English | [日本語](cli.ja.md)

`misao` is the command-line client of the daemon. It works from any SSH session, with or
without a controlling app running. Messages printed by the CLI are currently in Japanese;
this document describes behavior, options, and exit codes.

Related documents: [Protocol](protocol.md), [Configuration](config.md), [Embedding (Node SDK)](embedding.md).

```text
misao [options] <command> [args]
```

## Global options

Accepted by every command, before or after the command name.

| Option | Description |
|---|---|
| `--config PATH` | Use this `misao.json` (an error if it does not exist). See [file lookup](config.md#file-lookup). |
| `--json` | Machine-readable output. Errors are also JSON on stderr (see [Exit codes](#exit-codes)). |
| `--help` | Show help. After a command name, shows that command's usage. |
| `--version` | Show the version. |

Environment: `MISAO_CONFIG`, `MISAO_DIR`, `MISAO_SOCKET` select the config file and the
socket (see [Configuration](config.md)). Config warnings (unknown keys) go to stderr.

## Targeting panes

Commands that act on one pane take a `<target>`. It is resolved against `pane.list` in
**stages**: the first stage that matches at least one pane decides. If that stage matches
exactly one pane, it is the target; if it matches several, the command fails with an
"ambiguous" error that lists the candidates; if no stage matches, it fails with "not found".

| Stage | Form | Matches |
|---|---|---|
| 1 | `p_01M3...` (full id) | exact pane id |
| 2 | `task:N` | `task` label (a leading `#` is ignored) |
| 3 | `agent:ID` | `agent` label |
| 4 | `806` or `W-806` | `windowId` label (the hub's window number; `W-` is case-insensitive) |
| 5 | `7Q`, `p_01M3`, ... | prefix or suffix of the pane id (case-insensitive, `p_` optional), **2 characters or more** |
| 6 | any text | substring of the `name` label or the window name (case-insensitive) |

Rules worth knowing:

- A query of **digits only** (optionally `W-` first) is a window number and is decided at
  stage 4 only; it never falls through to ids or names. A pane id fragment that is only digits would
  be read as a window number, so write it **with `p_`** (for example `p_12`).
- `misao ls` shows ids as `p_01M3…7Q` (first 4 characters, `…`, and the shortest unique tail
  of at least 2 characters). The tail alone (`7Q`) works as a target.
- In tables the NAME column shows `W-806 · display name` for panes registered by a hub (they
  carry the `windowId` label) and `(未登録) ...` for unregistered ones, such as panes made with
  `misao new`. See [Label conventions](embedding.md#label-conventions).

## Commands

| Command | Purpose |
|---|---|
| `ls` | List panes (blocked first) |
| `attach` | Enter a pane |
| `new` | Create a pane |
| `kill` | Close a pane, a window, or a workspace |
| `send` | Send text or keys to a pane |
| `screen` | Print a pane's current screen text |
| `tail` | Follow a pane's output line by line |
| `events` | Follow daemon events |
| `label` | Set or remove labels |
| `status` | Show daemon status |
| `schema` | Print the protocol JSON schema |
| `serve` | Run the daemon in the foreground |

### ls

```text
misao ls [--state blocked|working|idle|exited] [--task N] [--workspace W] [--json]
```

Columns: `STATE PANE NAME TASK AGENT CWD LAST`. Order: blocked, working, idle, exited, stopped,
unknown; within a state, newest output first. `--state` filters by agent state; `--task` by
the `task` label (`#` optional); `--workspace` by workspace name. `--json` prints the array
of pane info objects (see [Protocol](protocol.md#methods)).

```bash
misao ls --state blocked
misao ls --task 439 --json
```

### attach

```text
misao attach <target> [--readonly] [--no-replay]
```

Connects your terminal to a pane. Needs a TTY (otherwise exit code `2`, before any connection).
It replays a screen snapshot first; `--no-replay` goes live only. `--readonly` sends no input and
does not resize. See [Keys in attach](#keys-in-attach). Input bytes are sent with
`source: "terminal"`; the daemon records only the count, never the content.

### new

```text
misao new [--cwd DIR] [--label k=v ...] [--env K=V ...] [--workspace W] [--window NAME] [--attach | --json] [-- cmd ...]
```

Creates a pane. Without `cmd` it runs your login shell with `-l`. The shell comes from the OS
user database, not from `$SHELL`; if it cannot be determined (no shell entry, or the user
database cannot be read), the command fails with a usage error (exit `2`). `--label` and `--env` can be
repeated. The pane always gets `origin=terminal`, which marks it as unregistered. Without
`--workspace` / `--window` the daemon's default window is used; with one of them the other
defaults to `default`, and missing workspaces and windows are created. `--attach` enters the
pane right away; it cannot be combined with `--json` (exit `2`).

`--env` values are saved to `persistence.json`. Do not pass secrets this way; use
`ephemeralEnv` through the protocol or SDK (see [Embedding](embedding.md#passing-secrets)).

```bash
misao new --label task=439 --label agent=claude -- claude
misao new --cwd ~/work --attach
```

### kill

```text
misao kill <target> [--window | --workspace] [--force] [--json]
```

Closes the target pane; `--window` closes the pane's whole window, `--workspace` its whole
workspace (with all panes). It asks `[y/N]` unless `--force`. Without a TTY and without `--force`
it refuses (exit `2`); answering no exits `1`.

### send

```text
misao send <target> [text] [--enter] [--keys Enter,Escape,C-c] [--stdin] [--json]
```

Writes bytes to the pane (`pane.write`). `text` is sent as typed; put a text that starts with
`-` after `--`. `--keys` takes comma-separated names: `Enter`, `Escape`, `Tab`, `Space`,
`Backspace`, `Delete`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, and
`C-<char>` (control keys). `Return` and `Esc` are aliases of `Enter` and `Escape`; names are
case-insensitive. `--enter` appends Enter. `--stdin` reads the text from stdin and cannot
be combined with `text`. Order: text, then `--keys`, then `--enter`. Something to send is required.

```bash
misao send W-806 'run the tests' --enter
misao send 7Q --keys Escape,C-c
echo 'hello' | misao send task:439 --stdin --enter
```

### screen

```text
misao screen <target> [--lines N] [--json]
```

Prints the current screen text without trailing blank lines; `--lines N` keeps the last N lines.
`--json` prints `text`, `cursor`, `altScreen`, `title`, and `activity`.

### tail

```text
misao tail <target> [--since SEQ [--epoch EPOCH]] [--json]
```

Follows the line stream until Ctrl-C (exit `0`). By default it first replays the retained lines
and then follows. `--since` resumes after a `seq` (see [Sequencing](protocol.md#sequencing-seq-epoch-since-gap-head));
without `--epoch` the seq is assumed to belong to the current epoch and a warning says so. `--epoch` needs
`--since`. `--json` prints one JSON object per line with `seq`, `ts`, `paneId`, `text`, and `epoch`.

The follower survives disconnects: the SDK reconnects and resumes, and progress and gaps are
reported on stderr.

### events

```text
misao events [--since SEQ [--epoch EPOCH]] [--pane <target>] [--json]
```

Follows daemon events until Ctrl-C. Live only unless `--since` is given. `--pane` keeps only
events of one pane. Text output: `seq  ts  type  paneId  data`; `--json` adds `epoch`.

### label

```text
misao label <target> [k=v ...] [--unset k ...] [--json]
```

Sets labels (`k=v`; the value may be empty) and removes keys with `--unset`, then prints the
labels after the change (`k=v` lines, or `{"labels": {...}}`).

### status

```text
misao status [--json]
```

Prints protocol version, pid, epoch, uptime, pane count, and socket. Exit code `0` when the daemon
runs, `1` when it is not reachable (`--json`: `{"running": false, "socket": ...}`).

### schema

```text
misao schema
```

Prints the result of `server.schema`: the JSON schema of all methods, notifications, events,
and error codes.

### serve

```text
misao serve [--socket PATH] [--data DIR]
```

Runs the daemon in the foreground until `SIGINT`, `SIGTERM`, or `SIGHUP`, then closes the panes
and exits. Settings come from `misao.json`; `--socket` overrides the socket path and `--data`
the directory of `daemon.pid` and `persistence.json` (default: the socket's directory). To
run it as a service see [deploy/README.md](../deploy/README.md).

## Keys in attach

While attached, every key goes to the pane except the **prefix** (default `Ctrl-^`) and the
key that follows it. The keys are configurable in [`keys`](config.md#schema).

| Keys | Action |
|---|---|
| `Ctrl-^` `d` | Leave the pane and show the pane list |
| `Ctrl-^` `n` | Go to the next pane (the order of `misao ls`, wrapping around) |
| `Ctrl-^` `p` | Go to the previous pane |
| `Ctrl-^` `l` | Show the pane list |
| `Ctrl-^` `Ctrl-^` | Send one `Ctrl-^` byte to the pane |
| `Ctrl-^` and any other key | The key is discarded |

On entering, a banner on stderr shows the display name (the NAME column of `misao ls`), task,
foreground command, state, `READONLY` with `--readonly`, and the key hints. The pane list takes a number to enter a pane, `n` for a new shell pane (not with
`--readonly`), and `q` to quit (exit `0`). Exited and stopped panes are not listed.
When the pane you are in exits, the CLI reports the exit code and returns to the list; if no
pane is left, it exits `0`. Entering a pane named on the command line that has already
exited is an error. If the connection is lost, or the CLI receives a termination signal
(`SIGTERM`, `SIGHUP`, `SIGINT`, `SIGQUIT`) inside a pane or at the pane list, it restores the
terminal and exits `1`. `attach` does not reconnect by itself.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success (including `attach` ended from the pane list, and `tail` / `events` ended with Ctrl-C) |
| `1` | Failure: not found, ambiguous target, daemon unreachable, RPC error, config error, aborted, runtime error; also `status` when the daemon is not running |
| `2` | Usage error: unknown command, bad or missing arguments, no TTY for `attach`, incompatible options |

With `--json`, an error is printed to stderr as one JSON object:

```json
{"error":{"code":"ambiguous","message":"...","candidates":[{"paneId":"p_01M3...","name":"W-806"}]}}
```

`code` is one of `usage`, `not_found`, `ambiguous`, `aborted`, `daemon_unreachable`, `rpc`,
`config`, `runtime`. `candidates` appears only for `ambiguous`.
