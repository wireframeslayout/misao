# Embedding

English | [日本語](embedding.ja.md)

How to drive misao from your own Node.js program with the SDK (`@misao/sdk`), and the
conventions an embedding app should follow (labels, secrets, agent profiles). The wire
semantics (`seq`, `epoch`, `gap`, pull-model subscriptions, activity detection) are
defined once in [Protocol](protocol.md) and only linked from here.

Related documents: [Protocol](protocol.md), [Configuration](config.md), [CLI](cli.md).

The packages are ESM and need Node.js 24 or later (declared in `engines` of the repository root
`package.json`).

## Installing

`@misao/sdk` and `@misao/protocol` are not published to npm. Each release attaches one tarball
per package to its [GitHub Release](https://github.com/wireframeslayout/misao/releases), and you
depend on the tarball URLs. The SDK imports types from `@misao/protocol`, so list both. In your
`package.json` (replace `0.1.0` with the release you want, in the tag and in the file names):

```json
{
  "dependencies": {
    "@misao/sdk": "https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-sdk-0.1.0.tgz",
    "@misao/protocol": "https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-protocol-0.1.0.tgz"
  }
}
```

- The tarball of `@misao/sdk` already points `@misao/protocol` at the protocol tarball of the same
  release, so listing only the SDK also installs. Listing both keeps the two versions visibly in
  sync and lets you bump them together.
- The tarballs contain only compiled JavaScript (`dist`) and type declarations. Their types refer
  to Node.js types, so TypeScript projects need `@types/node` as a dev dependency.
- A tag that contains a hyphen (for example `v0.2.0-rc.1`) is a pre-release.
- Tarballs of a published tag are never replaced. A fix ships as a new tag.
- To build the same tarballs locally: `npm run clean && npm run build`, then
  `node scripts/set-version.mjs <version>` and `npm run release:pack -- --tag v<version>` (output
  in `release/`), then `node scripts/verify-release.mjs --tag v<version>`, which installs the SDK
  tarball into a temporary project and checks the runtime import and type resolution. Undo the
  version change afterwards with `git checkout -- package.json package-lock.json packages/*/package.json`.

## Bundling the daemon and CLI

Each release also attaches `misao-<version>.mjs`: the CLI and the daemon bundled into one ESM
file, for apps that ship misao themselves (for example AZITO). `SHA256SUMS` in the same release
lists the sha256 of every asset (both tarballs and the bundle); check it with
`sha256sum -c --ignore-missing SHA256SUMS`.

```bash
node misao-0.1.0.mjs --version
node misao-0.1.0.mjs serve
```

- `misao-<version>.LICENSES.txt` (also in `SHA256SUMS`) holds the license text of misao
  (Apache-2.0, with `NOTICE`) and of every third-party package inside the bundle (`zod`,
  `@xterm/headless`, both MIT). Ship it with the bundle. The first lines of the bundle point to it.
  `@xterm/headless` publishes no license file, so only its declared license and repository are listed.
- `node-pty` (a native module) is **not** inside the bundle. The bundle resolves it with the normal
  Node.js lookup, so put it in a `node_modules/node-pty` next to the file (or in any parent
  directory). An embedding app that already ships `node-pty ^1.1.0` shares its copy; misao does not
  ship prebuilt binaries itself. `@xterm/headless` and `zod` are inside the bundle.
- The bundle is ESM and needs Node.js 24 or later. The first line is a `#!/usr/bin/env node`
  shebang, so it can also be made executable and run directly.
- The CLI and the daemon are the same file, so `--version` and the `version` field of `server.info`
  are the release version baked into that file (`misao 0.1.0` and `"version": "0.1.0"`). Unlike
  `protocolVersion`, it tells apart two daemons that speak the same protocol but are different
  builds. An older daemon that does not send `version` is reported as such by its absence. To run the
  daemon as a service, see [deploy/README.md](../deploy/README.md) (systemd unit for Linux, launchd
  plist for macOS).
- Compatibility: the bundle's daemon reports `PROTOCOL_VERSION` in `server.info`, and a client
  (`@misao/sdk`) is compatible when the **major** versions are equal (see
  [Protocol](protocol.md#versioning-and-compatibility)). Ship an SDK and a bundle from the same
  release, or at least with the same protocol major. A minor difference is allowed: unknown fields
  and enum values are tolerated on the receiving side.
- To build it locally: `node scripts/bundle-cli.mjs --out dist-release`, then
  `node scripts/smoke-bundle.mjs dist-release/misao-<version>.mjs`. The smoke test copies the
  bundle and `node-pty` to a temporary directory and checks `--version` and `server.info` against the file name's version, `serve` on a temporary
  socket, `server.info` (its `version` must equal the file name's version), creating, reading, and
  killing one pane (proves the external `node-pty` works), and a clean stop. It never touches a running daemon.

## Connecting

```ts
import os from 'node:os';
import { MisaoClient, resolveSocketPath } from '@misao/sdk';

const socketPath = resolveSocketPath({ env: process.env, homeDir: os.homedir() });
const client = new MisaoClient({ socketPath });
await client.connect();

const panes = await client.request('pane.list', {});
client.close();
```

- `resolveSocketPath` applies the same order as the CLI: `$MISAO_SOCKET`, then `explicitPath`
  (for example the `socket` of `misao.json`), then `$MISAO_DIR/misao.sock`, then
  `~/.misao/misao.sock`. A path longer than `MAX_SOCKET_PATH_BYTES` (107 bytes) throws
  `MisaoPathError`. See [Socket location](config.md#socket-location).
- `MisaoClientOptions`: `socketPath` (required), `backoff` (partial, see below), and
  `connectTimeoutMs` (default `5000`).
- `connect()` connects, calls `server.info`, checks that the protocol major versions match, and
  restores streams. It rejects when the daemon is unreachable (`MisaoConnectionError`) or
  incompatible (`MisaoProtocolVersionError`, see [Versioning](protocol.md#versioning-and-compatibility)).
  It can be called only while the client is idle: before the first attempt, or after a failed one.
- `connectTimeoutMs` bounds the setup after the socket connects (the `server.info` check and
  stream restore). On timeout the connection is closed and `connect()` (or the current reconnect
  attempt) fails with `MisaoConnectionError`. This protects against a hung daemon.
- `request(method, params)` is typed by `@misao/protocol`. A daemon error rejects with
  `MisaoRpcError` (`code`, `message`; see [Errors](protocol.md#errors)). Calling it while not
  connected rejects with `MisaoConnectionError`. `params` are validated by the schema before
  sending; invalid params reject with a Zod `ZodError` and nothing is sent.
- `close()` ends the client and stops reconnecting.

## Reconnect and backoff

After a successful `connect()`, a lost connection is retried until you call `close()`.

| Option (`backoff`) | Default | Meaning |
|---|---|---|
| `initialDelayMs` | `100` | Delay before the first retry |
| `maxDelayMs` | `5000` | Upper bound of the delay |
| `factor` | `2` | Multiplier per attempt |

The delay before attempt `n` (starting at 1) is `min(maxDelayMs, initialDelayMs * factor^(n-1))`
(`computeBackoffDelay`). There is no attempt limit. The only reasons for the client to give up
are `close()` and a daemon that turns out to be protocol-incompatible after a restart; either
way the state becomes `closed`, and `cause` is set only for the incompatible daemon (an unexpected
internal error in the reconnect loop is reported to `onError` and also ends in `closed`). Requests in flight at the time of the disconnect reject
with `MisaoConnectionError`.

```ts
client.onStateChange((state) => {
  // { status: 'connected' }
  // { status: 'reconnecting', attempt, delayMs, cause }
  // { status: 'closed', cause? }
});
```

`connected` is also emitted once at the end of the first `connect()`, before its promise
resolves, and again after each successful reconnect. To receive the first one, register the
listener before calling `connect()`.

## Following streams

```ts
import { parseKnownEvent } from '@misao/protocol';

const lines = await client.subscribeLines(paneId, (line) => {
  console.log(line.seq, line.text);
});

const events = await client.subscribeEvents((event) => {
  const known = parseKnownEvent(event);      // undefined for event types this version does not know
  if (known?.type === 'pane.state') console.log(known.paneId, known.data.state);
});

// Later, for example before a process restart:
const saved = lines.cursor;                  // { seq, epoch }
lines.unsubscribe();
```

- With no options a subscription is live only. To resume, pass **`since` and `epoch` together**:
  `{ since: saved.seq, epoch: saved.epoch }`. The type forbids passing only one, because without
  the `epoch` the daemon cannot tell whether `seq` was rewound by a restart.
  `Subscription.cursor` always returns the latest position and can be stored as is.
- While connected, the SDK keeps the last `seq` of each stream, drops duplicates, and after a
  reconnect re-subscribes every stream with that `since` and the `epoch` it saw. You do not
  re-subscribe by hand.
- If the daemon's `epoch` changed (the daemon restarted), the SDK resets the position to `0`; the
  daemon replays from the oldest retained item, and `onGap` fires with reason `epoch`.
- If the position fell out of the ring (or the SDK was disconnected because the ring overtook it
  — see the [pull model](protocol.md#subscriptions-pull-model-and-back-pressure)), the daemon
  returns `gap: true` and `onGap` fires with reason `truncated`.
- Only one subscription per stream (events, or lines of one pane) can exist; a second one
  rejects with a plain `Error` (`stream already registered: <key>`).
- Handler exceptions do not stop delivery; they are reported to `onError`.

## Callbacks

Each `on...` method returns a function that removes the callback.

| Method | Called when | Argument |
|---|---|---|
| `onStateChange` | Connected, reconnecting, or closed | `ConnectionState` |
| `onGap` | Items were missed on a stream | `{ stream, reason }`, `stream` is `{ kind: 'events' }` or `{ kind: 'lines', paneId }`, `reason` is `'epoch'` or `'truncated'` |
| `onSubscriptionError` | The daemon rejected a re-subscribe (for example the pane is gone). The stream is dropped. | `{ stream, error: MisaoRpcError }` |
| `onNotification` | A notification that is not a stream subscription, namely `pane.output` | `{ method, params }` |
| `onError` | A callback or handler you registered threw | `unknown` |

How to recover from `onGap`: the missed output is gone from the stream, so rebuild state from a
snapshot (for example `pane.screen`, or `pane.info`) and continue from the new position. Inside
the callback `client.request` already works, because the connection is usable before streams are
restored.

`onError` is the single place for exceptions thrown by your callbacks; they never stop
reconnecting, delivery, or gap notifications. With no `onError` listener, the SDK prints them to
`console.error`.

## Re-attaching after reconnect

`pane.attach` (raw output) has no `since`, so the SDK does **not** restore it. After every
reconnect you must attach again. Register the output handler once; `onNotification` listeners
survive reconnects.

```ts
const clientId = 'my-app-1';

client.onNotification((n) => {
  if (n.method === 'pane.output' && n.params.paneId === paneId) {
    term.write(Buffer.from(n.params.dataB64, 'base64'));
  }
});

const attach = (): Promise<unknown> =>
  client.request('pane.attach', { paneId, clientId, replay: 'snapshot', cols, rows });

await attach();
client.onStateChange((state) => {
  if (state.status === 'connected') attach().catch((error) => report(error));
});
```

- `replay: "snapshot"` sends one full screen first (`pane.output` with `replay: "snapshot"`):
  reset your terminal view before drawing it. `replay: "raw"` replays the raw ring instead
  (`truncated` tells you if it already lost bytes); `"none"` is live only.
- After a daemon restart the pane is `stopped` and attach answers `1002`; check `pane.info`
  first (see [Pane lifecycle](protocol.md#pane-lifecycle-and-persistence)).
- Attaching the same pane again on the same connection replaces the earlier attach.
- Send input with `pane.write` and `source: 'hub'` (your app) or `'terminal'`; the daemon records
  only the byte count.

## Passing secrets

`env` of `pane.open` is saved to `persistence.json`. Pass tokens and other secrets in
`ephemeralEnv`: it reaches the child process but is never persisted and never appears in
`pane.info`, `pane.list`, or events.

```ts
const { paneId } = await client.request('pane.open', {
  cmd: ['claude'],
  cwd: '/work/repo',
  env: { MY_APP_MODE: 'agent' },
  ephemeralEnv: { MY_APP_TOKEN: token },
  labels: { owner: 'my-app', task: '439', agent: 'claude', origin: 'hub' },
});
```

After a daemon restart the pane is `stopped` and the secret is gone; when respawning exists
(not implemented yet, `1005`) the caller must pass `ephemeralEnv` again. On the same key,
`ephemeralEnv` overrides `env`.

## Label conventions

Labels are free-form string pairs; **the daemon never interprets them**. They exist so that
apps and the CLI agree on how to describe a pane. Use `pane.set_label` or `labels` of
`pane.open`, and filter with `pane.list` (`filter.labels` matches when every key/value is equal).

| Key | Meaning | Example |
|---|---|---|
| `owner` | The app or user that owns the pane | `azito` |
| `task` | Task number the pane works on (`#` is optional; the CLI ignores a leading `#`) | `439` |
| `agent` | Identifier of the agent running in the pane | `claude` |
| `origin` | How the pane was created: `hub` (by the controlling app) or `terminal` (by a person with `misao new`) | `hub` |
| `windowId` | The **hub's window number**, e.g. `806`. The CLI shows it as `W-806` and accepts `806` / `W-806` as a [target](cli.md#targeting-panes). Unrelated to the daemon's window id (`w_...`). | `806` |
| `name` | Human-readable display name | `misao plan` |

Panes made by `misao new` always carry `origin=terminal`, so a hub can find panes that it has
not registered yet (no `windowId`) and offer to register them.

## Agent profiles

[Activity detection](protocol.md#activity-detection) has a plug-in point for agent-specific
screen rules. A profile decides `working`, `blocked`, or `idle` from the visible screen; it is
the only way to get `blocked`.

```ts
interface AgentProfile {
  readonly name: string;                              // becomes pane.state decidedBy
  matches(cmd: readonly string[]): boolean;           // chosen by pane.open's cmd; first match wins
  classify(screen: ProfileScreen): 'working' | 'blocked' | 'idle' | null; // null = no opinion
}
interface ProfileScreen { rows: readonly string[]; title: string; altScreen: boolean }
```

`classify` must be a pure function of the screen. Profiles are passed to the daemon through
`DaemonOptions.profiles`, so they are available when you embed the daemon in your own process:

```ts
import { Daemon } from '@misao/daemon';
import type { AgentProfile } from '@misao/daemon';

const myAgent: AgentProfile = {
  name: 'my-agent',
  matches: (cmd) => cmd[0]?.endsWith('my-agent') === true,
  classify: ({ rows }) => (rows.some((r) => r.includes('Proceed? [y/n]')) ? 'blocked' : null),
};

const daemon = new Daemon({
  socketPath,
  pidPath,
  statePath,          // persistence.json
  profiles: [myAgent],
});
await daemon.start();
```

`misao serve` does not take profiles; it runs with the generic title and bytes rules. Profiles for
Claude Code and Codex are not shipped yet.
