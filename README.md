# misao

**misao** (操, "to operate") is a headless terminal pane server for AI coding agents.

It hosts PTYs, keeps a screen model for each pane, and exposes everything over a local
Unix-socket API (NDJSON JSON-RPC 2.0). It has no terminal UI of its own: browsers, desktop
apps, TUIs, and SSH terminals are all clients.

> Status: **pre-alpha**. The Phase 0 feasibility spike lives on the
> [`spike/phase-0`](../../tree/spike/phase-0) branch. Phase 1 (daemon, protocol, CLI, Node SDK)
> is in progress on `main`.

## What it does

- Runs each pane on a real PTY (node-pty) and keeps its screen with `@xterm/headless`
- Serves three views of every pane: raw bytes for attach (with replay for late joiners),
  an ANSI-stripped line stream for marker detection, and screen snapshots
- Numbers every output line and event with a monotonic `seq`, so a client that was
  disconnected can resume with `since=<seq>` and learn when it missed something (`gap`)
- Detects agent activity (working / blocked / idle / exited) inside the daemon
- Keeps working when the controlling app is down: `misao ls / attach / new / kill / send`
  from any SSH session

## Packages (planned)

| Package | Purpose |
|---|---|
| `@misao/protocol` | JSON schema and TypeScript types for the socket API |
| `@misao/daemon` | PTY host, screen model, ring buffers, events |
| `misao` (cli) | `serve`, `ls`, `attach`, `new`, `kill`, `send`, `screen`, `tail`, `events` |
| `@misao/sdk` | Node client with reconnect and `since` tracking |
| `@misao/bridge`, `@misao/web`, `@misao/profile-*` | Later phases |

## Development

Requires Node.js >= 24.

```bash
npm ci          # install (builds node-pty natively)
npm run build   # tsc -b across all packages
npm run typecheck
npm test        # node:test via tsx, per workspace (runs against src, no build needed)
npm run smoke   # run the built CLI directly
```

During development run the CLI with `node packages/cli/dist/main.js`, or `npx misao`
(after `npm run build`, run `npm rebuild misao` once to create the bin link).

## License

[Apache License 2.0](LICENSE). Contributions require agreeing to the [CLA](CLA.md).

日本語: [README.ja.md](README.ja.md)
