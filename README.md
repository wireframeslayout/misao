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

`@misao/sdk` and `@misao/protocol` are not on npm. Depend on the tarballs attached to each
[GitHub Release](https://github.com/wireframeslayout/misao/releases), for example
`"@misao/sdk": "https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-sdk-0.1.0.tgz"`.
See [Installing](docs/embedding.md#installing).

## Development

Requires Node.js >= 24.

```bash
npm ci          # install (builds node-pty natively)
npm run build   # tsc -b across all packages
npm run typecheck
npm test        # node:test via tsx, per workspace (runs against src, no build needed)
npm run test:scripts  # node:test for the release scripts under scripts/
npm run smoke   # run the built CLI directly
npm run test:e2e  # end-to-end tests against a real daemon process (see below)
```

The e2e tests (`packages/e2e`) start `misao serve` from a temporary directory and drive it
with fake agents, a fake hub, vim, and bash. They stay out of `npm test`. Cases that need
external tools are opt-in: `MISAO_E2E_CLAUDE=1` (Claude Code), `MISAO_E2E_CODEX=1` (Codex),
`MISAO_E2E_SYSTEMD=1` (creates a temporary systemd user unit). `MISAO_E2E_FULL=1` restores
the full scale of the Phase 0 spike.

To run the daemon under systemd, see [deploy/README.md](deploy/README.md).

During development run the CLI with `node packages/cli/dist/main.js`, or `npx misao`
(after `npm run build`, run `npm rebuild misao` once to create the bin link).

## Documentation

English is the canonical version; each document has a Japanese edition (`*.ja.md`) next to it.

| Document | Contents |
|---|---|
| [docs/protocol.md](docs/protocol.md) | Socket protocol: methods, notifications, `seq` / `epoch` / `gap`, errors |
| [docs/cli.md](docs/cli.md) | The `misao` command: targets, commands, attach keys, exit codes |
| [docs/config.md](docs/config.md) | `misao.json`, socket location, memory sizing, running as a service |
| [docs/embedding.md](docs/embedding.md) | Node SDK, label conventions, secrets, agent profiles |

## License

[Apache License 2.0](LICENSE). Contributions require agreeing to the [CLA](CLA.md).

日本語: [README.ja.md](README.ja.md)
