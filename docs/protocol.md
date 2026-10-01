# Protocol

English | [日本語](protocol.ja.md)

The misao daemon serves a JSON-RPC 2.0 API over a local Unix socket. This document describes
the wire protocol: transport, versioning, methods, notifications, sequencing, subscriptions,
activity detection, and errors. The full machine-readable schema is the source of truth for
parameter and result shapes (see [Methods](#methods)).

Related documents: [Configuration](config.md), [CLI](cli.md), [Embedding (Node SDK)](embedding.md).

## Transport

- A Unix domain socket (default `~/.misao/misao.sock`, see [Socket location](config.md#socket-location)).
- NDJSON: one JSON-RPC 2.0 message per line, terminated by `\n`. Blank lines are ignored.
- A line is limited to 8 MiB. A longer line gets a `-32700` error with `id: null` and the
  connection is closed.
- Three message kinds: requests (`id` + `method`), responses (`id` + `result` or `error`), and
  server notifications (`method` without `id`). The daemon ignores notifications and responses
  sent by clients.
- A response carries exactly one of `result` or `error`. A method with nothing to return
  answers `null` or `{ "ok": true }`.

```json
{"jsonrpc":"2.0","id":1,"method":"server.info","params":{}}
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"0.2.0","pid":4242,"epoch":"01M3RCFVKNN4GWG2FBNS6BVTW2","uptimeSec":12,"paneCount":3,"eventHead":57}}
```

The socket directory is created with mode `700` and the socket with mode `600`; access control
is the filesystem permission of the current user.

## Versioning and compatibility

`PROTOCOL_VERSION` (currently `0.2.0`) is semver and is reported by `server.info` and the
`daemon.started` event.

- **Compatible** means the **major** versions are equal (`isCompatibleProtocolVersion`).
  The SDK checks this right after connecting and rejects with `MisaoProtocolVersionError` when
  it does not hold.
- Adding fields, methods, event types, or enum values is a minor change and backward compatible.
  Removing or changing the meaning of something is a major change.

Forward-compatibility rules (what a peer must tolerate):

| Direction | Rule |
|---|---|
| Request `params` (client to daemon) | Validated by the schema. **Unknown keys are stripped** (ignored). Values must be valid: an unknown enum value is rejected with `-32602`. The one exception is the `pane.list` `filter`, which rejects unknown conditions so that an old daemon never ignores a filter and returns every pane. |
| `result` and notification `params` (daemon to client) | **Loose**: unknown fields must be ignored. |
| Enum values in `result` / notifications / event `data` | Receivers tolerate unknown values. The schemas fall back to `"unknown"` (`processState`, `agentState`, event `input.source`, `pane.output` `replay`). The daemon itself never sends `"unknown"` for `processState`, `input.source`, or `replay`. |
| Event `type` | An unknown `type` is not an error. `parseKnownEvent(event)` validates `data` of known types and returns `undefined` for unknown types. |

Note: `agentState` is a state the daemon computes, and `unknown` is also a real value: it is the
initial state of a pane before any rule has an opinion (see [Activity detection](#activity-detection)).

Reserved but not available:

- `pane.attach` with `mode: "cells"` returns `1004` (Unsupported). Only `mode: "raw"` works.
- `pane.respawn` exists in the schema only and returns `1005` (NotImplemented).
- `window.focus` exists in the schema only and returns `1005` (NotImplemented). The `focus`
  event is defined in the schema but never emitted yet.

## Methods

Parameters and results are summarized here; they are not repeated in full. The complete JSON
schema (methods, notifications, events, error codes) is available from the daemon:

```bash
misao schema          # prints the result of server.schema as JSON
```

or as the `server.schema` method. Unless noted, a method that takes a `paneId` answers `1001`
when the pane does not exist. `pane.detach` is the exception: it answers `ok` even when the pane
or the attachment does not exist.

| Method | Purpose | Key params | Result |
|---|---|---|---|
| `server.info` | Daemon identity and stream heads | none | `protocolVersion`, `pid`, `epoch`, `uptimeSec`, `paneCount`, `eventHead` |
| `server.schema` | JSON schema of the whole protocol | none | `{ protocolVersion, jsonrpc, methods, notifications, events, errors }` |
| `workspace.list` / `workspace.create` / `workspace.close` / `workspace.rename` | Manage workspaces | `name` (`newName` for rename) | workspace info (`name`, `windows`) or `ok` |
| `window.create` / `window.close` / `window.rename` | Manage windows inside a workspace | `workspace`, `name`, `windowId` | window info or `ok` |
| `window.focus` | Schema only | `windowId`, `clientId` | always `1005` |
| `pane.open` | Start a command on a new PTY | `cmd` (required), `cwd`, `env`, `ephemeralEnv`, `cols`, `rows`, `labels`, `windowId` | `{ paneId }` |
| `pane.info` / `pane.list` | Read pane state | `paneId`; `filter` (`state`, `labels`, `workspace`, ANDed) | pane info (one or array) |
| `pane.write` | Write bytes to the PTY | `paneId`, `data` (UTF-8) **or** `dataB64`, `clientId`, `source` (`hub` / `terminal`) | `ok` |
| `pane.send_keys` | Schema only | `paneId`, `keys` | always `1005`; use `pane.write` |
| `pane.resize` | Resize the PTY | `paneId`, `cols`, `rows`, `clientId` | `ok` |
| `pane.screen` | Current screen text | `paneId` | `text`, `cursor`, `altScreen`, `title`, `activity` |
| `pane.set_label` | Set / unset labels | `paneId`, `set`, `unset` (at least one) | `{ labels }` after the change |
| `pane.attach` | Stream raw output to this connection | `paneId`, `clientId`, `mode` (default `raw`), `replay` (`raw` / `snapshot` / `none`, default `none`), `cols`, `rows` | `head`, `oldest`, `truncated` |
| `pane.detach` | Stop streaming raw output to this connection | `paneId` | `ok` |
| `pane.subscribe_lines` | Subscribe to the line stream | `paneId`, `since`, `epoch` | `gap`, `head`, `epoch` |
| `events.subscribe` | Subscribe to daemon-wide events | `since`, `epoch` | `gap`, `head`, `epoch` |
| `pane.close` | Close a pane (SIGHUP, wait for exit) | `paneId` | `ok` |
| `pane.respawn` | Reserved | `paneId`, `cmd` | always `1005` |

Notes:

- `pane.open` accepts `preplace` in the schema, but the daemon answers `1005` when it is given.
- `pane.write` records an `input` event with the source and byte count only; the content is never recorded.
- `pane.list` `filter.labels` matches when every given key/value is equal.
- `pane.attach` with `replay: "raw"` replays the retained raw ring (`truncated: true` if bytes were
  already dropped); `replay: "snapshot"` sends one screen snapshot first (a `pane.output`
  notification with `replay: "snapshot"`); `none` goes live only. After the replay the stream
  switches to live output without a gap at `head`.
- Pane ids are `p_` + ULID, window ids `w_` + ULID (daemon-side ids).

## Notifications

All notifications are server-to-client and carry `seq` and `ts` (ISO 8601 UTC, recorded by the daemon).

| Notification | Delivered after | Params |
|---|---|---|
| `pane.output` | `pane.attach` | `seq`, `ts`, `paneId`, `dataB64` (raw PTY bytes), optional `replay` (`"snapshot"`) |
| `pane.line` | `pane.subscribe_lines` | `seq`, `ts`, `paneId`, `text` (one line, ANSI sequences removed) |
| `event` | `events.subscribe` | `seq`, `ts`, `type`, optional `paneId`, `data` |

Known event types (the `data` schema of each is in `server.schema`):

| Group | Types |
|---|---|
| daemon | `daemon.started` |
| pane | `pane.opened`, `pane.title`, `pane.exited`, `pane.resized`, `pane.closed`, `pane.state`, `pane.label` |
| input / clients | `input` (source and byte count only), `client.attached`, `client.detached` |
| layout | `workspace.created`, `workspace.closed`, `workspace.renamed`, `window.created`, `window.closed`, `window.renamed`, `focus` (reserved, not emitted yet) |

Events tied to a pane carry `paneId` next to `data`, not inside it.

## Sequencing: seq, epoch, since, gap, head

There are three independent streams:

1. raw output, per pane (`pane.output`; `seq` counts chunks),
2. lines, per pane (`pane.line`),
3. daemon-wide events (`event`).

| Term | Meaning |
|---|---|
| `seq` | Per-stream counter that increases by 1 from 1. An empty stream has `head = 0`. |
| `head` | The latest `seq` at subscribe time. Replay covers `seq <= head`, live delivery covers `seq > head`. |
| `oldest` | The oldest `seq` still retained in the ring (`head + 1` when empty). |
| `since` | The last `seq` the client already has. The daemon replays `seq > since`, then goes live. Omit it for live only. |
| `epoch` | A ULID generated on every daemon start. Reported by `server.info` and in every subscribe result. |
| `gap` | `true` in the subscribe result when the client missed items. |

A subscription is resumed by passing `since` **together with** the `epoch` it belongs to:

- `gap` is `true` when `since < oldest - 1` (the ring dropped items the client never saw),
  when `since > head` (the daemon restarted and `seq` rewound), or when `since` is given and the
  passed `epoch` differs from the current one. The `epoch` is compared only when `since` is
  given; an `epoch` without `since` is a live-only subscription with `gap: false`.
- When `since` is given and the passed `epoch` differs from the current epoch, the daemon treats `since` as `0`:
  it replays from the oldest retained item and returns `gap: true`. The client must discard its
  stored `since`. A daemon restart is reliably detected only by comparing `epoch`; `since > head`
  is a secondary signal.
- When `since` is older than the ring, the daemon replays from `oldest` and returns `gap: true`.
- If `epoch` is omitted, the daemon cannot compare generations and falls back to the `since`
  checks above. Always send them as a pair (the SDK's types enforce this).
- A new subscription for the same `(stream, pane)` on the same connection replaces the old one,
  so a client never receives duplicates.
- The daemon sends the subscribe response before the replay notifications on the same connection.
  Clients must process the response first (it sets `head` and `gap`).

Ring capacities (per daemon start; see [Configuration](config.md#schema)):

| Ring | Capacity unit | Default |
|---|---|---|
| raw output (per pane) | bytes | 1 MiB |
| lines (per pane) | bytes (characters + 64 per line) | 4 MiB |
| events (daemon-wide) | items | 1000 |

A ring always keeps at least the latest item, even when that single item exceeds the capacity.

## Subscriptions: pull model and back-pressure

Line and event subscriptions are **pull-based**. Each subscription keeps its own cursor (the
last `seq` sent) and reads from the ring only while the connection's send queue is below
1 MiB; it continues when new items arrive and when the socket drains. A slow subscriber is
therefore absorbed by the ring, not by memory in the send queue.

- A subscriber is disconnected **only when the ring overtook it**: the item after its cursor
  was already evicted. This is checked even while the socket is not writable, so a client that
  does not read at all is detected too.
- The send-queue limit of 16 MiB applies **only to `pane.attach` raw output and to responses**.
  A connection whose queue exceeds it is closed. Line and event subscriptions do not use it.
- When a subscriber is overtaken and disconnected, the SDK reconnects and re-subscribes with
  its last `since` and `epoch`. The daemon answers `gap: true` (no epoch change), and the SDK
  reports `gap` with reason `truncated`. See [Embedding](embedding.md#following-streams).
- Using `pane.attach` and a subscription on the same connection: while raw output keeps the
  queue above the high-water mark, the subscription also pauses.

Measured behaviour (an indication only; it depends on the machine and Node version):

| Measurement | Result |
|---|---|
| Before the pull model, burst test of 100 rounds | 6 disconnects; 57 of 100 markers detected |
| Pull model, default 4 MiB line ring, burst test of 1000 rounds | 0 disconnects, 0 gaps, 1000 of 1000 markers detected, about 57,000 lines per second |
| Phase 0 spike, burst of 1000 rounds | 10,005,000 lines in 165 s, `seq` contiguous, 1000 of 1000 markers detected |
| Phase 0 spike, subscriber that stopped reading for long | told `gap: true` on resume; a client resuming within the ring saw a contiguous `seq` |

## Pane lifecycle and persistence

- `pane.open` spawns `cmd` on a PTY. The child environment is the daemon's environment with
  `TMUX`, `TMUX_PANE`, `STY`, and `ZELLIJ*` removed, plus `MISAO_SOCKET`, `MISAO_PANE_ID`,
  `TERM=xterm-256color`, and `COLORTERM=truecolor`, then `env`, then `ephemeralEnv`.
- A pane that has exited stays listed with `processState: "exited"`, `exitCode`, and `signal`
  until it is closed. `pane.close` removes it.
- **`fgCommand`**: only for a pane with `processState: "running"`, `pane.info` / `pane.list` report
  the command name of the pane's foreground process group. It is computed on each call. For an
  interpreter (`node`, `bun`, `python`, `ruby`, `perl`) it is the script name without its
  extension. It is a display name: control characters are removed and it is at most 64
  characters. It is omitted when it cannot be read. On non-Linux platforms it is the value of
  node-pty's `process`.
- **`ephemeralEnv`**: environment variables injected into the child like `env`, but never saved
  to `persistence.json` and never shown in `pane.info`, `pane.list`, or events. Use it for tokens
  and other secrets; `env` is persisted. On the same key it overrides `env`. After a daemon
  restart the pane is `stopped` and the values are gone, so a future respawn must pass them again.
- **`persistence.json`**: the daemon saves workspaces, windows, and pane records (`cmd`, `cwd`,
  `env`, `labels`, size) to `persistence.json` in the data directory (by default the socket's
  directory), atomically with `fsync`. Size changes are saved with a short delay; definition and
  label changes are saved before the response. A corrupt file or an unknown `version` stops the
  daemon from starting.
- **`stopped`**: when the daemon restarts, PTYs are lost; only metadata remains. Restored panes
  appear with `processState: "stopped"`, `pid: null`, `agentState: "unknown"`, and
  `decidedBy: "none"`. Input, resize, screen, attach, and line subscription on a stopped pane
  answer `1002`. `pane.close` removes the record. Restarting a pane from its record
  (`pane.respawn`) is not implemented yet (`1005`).
- The daemon's `epoch` changes on every start, so clients detect the restart (see above).

## Activity detection

The daemon computes `agentState` per pane and reports it in `pane.info` / `pane.list` and as
`pane.state` events (`state`, `decidedBy`, `prev`). Rules are checked in this priority order and
the first rule that has an opinion decides:

| Priority | Rule (`decidedBy`) | Decides |
|---|---|---|
| 1 | exit (`exit`) | `exited` when the process ended. Terminal; nothing is evaluated afterwards. |
| 2 | profile (the profile's `name`) | Agent-specific screen rules. The only rule that can say `blocked`. |
| 3 | title (`title`) | OSC window title: a spinner title (braille, `◐◑◒◓`, `✻✶✽✢∗` followed by a space) is `working` while it keeps updating (stale after 3 s, then no opinion); once a spinner has been seen, a non-empty non-spinner title is `idle`. |
| 4 | bytes (`bytes`) | Output volume, checked on a 1-second tick. A tick is active when new output arrived since the previous tick and the last 3 s hold at least 200 bytes. Two active ticks in a row give `working`; 5 s after the last active tick gives `idle`. Output right after input (500 ms) or resize (800 ms) is not counted. |

- The core produces only `working`, `idle`, and `exited`. `blocked` comes only from a profile.
- The initial state is `agentState: "unknown"`, `decidedBy: "none"`. It stays until a rule has an opinion
  (for the bytes rule, that is at the earliest 5 s after opening, or two active ticks).
- If no rule has an opinion, the state stays as it is.
- Profiles are pure `classify(screen)` functions selected by the first `matches(cmd)`. They are plugged
  into the daemon through `DaemonOptions.profiles`; see
  [Embedding](embedding.md#agent-profiles). A profile that throws three times in a row is
  disabled for that pane (a warning is logged), and a single failure counts as no opinion.
- The profile is evaluated after output settles (120 ms debounce, at most 300 ms after the first output).

## Errors

An error response has the form `{ "code": <int>, "message": <string>, "data"?: ... }`. Messages
are for humans and never include stack traces; match on `code`.

| Code | Name | Meaning |
|---|---|---|
| -32700 | Parse | The line is not valid JSON, or exceeds the 8 MiB line limit (connection is then closed). |
| -32600 | InvalidRequest | Not a JSON object or not a valid JSON-RPC request. |
| -32601 | MethodNotFound | Unknown method. |
| -32602 | InvalidParams | `params` failed validation (the message lists `path: reason`). Also returned by `pane.open` when the process cannot be spawned. |
| -32603 | Internal | Unexpected daemon error. |
| 1001 | PaneNotFound | No pane with that id. |
| 1002 | PaneExited | The pane has exited (write) or is `stopped` (any operation that needs a live process). |
| 1003 | Ambiguous | Reserved. The daemon does not return it; the CLI resolves targets on the client side. |
| 1004 | Unsupported | `pane.attach` with `mode: "cells"`. |
| 1005 | NotImplemented | `pane.respawn`, `pane.send_keys`, `window.focus`, and `preplace` of `pane.open`. |
| 1006 | WorkspaceNotFound | Workspace does not exist (or is being closed). |
| 1007 | WindowNotFound | Window does not exist (or is being closed). |
| 1008 | AlreadyExists | A workspace with that name already exists. |

The SDK surfaces these as `MisaoRpcError` with `code`; see [Embedding](embedding.md).
