#!/usr/bin/env node
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from './daemon/Daemon.js';
import { MisaoClient } from './client/MisaoClient.js';
import { attach } from './cli/attach.js';
import type { PaneInfo } from './daemon/Pane.js';

const HELP = `usage: misao-spike [--dir DIR] <command>
  serve
  status
  ls [--json]
  open [--cwd D] [--label k=v] -- cmd...
  send <pane> <text> [--enter]
  screen <pane>
  tail <pane> [--since N] [--follow]
  events [--since N] [--follow]
  attach <pane> [--replay raw|snapshot|none] [--readonly]
  close <pane>
`;

interface Parsed {
  positional: string[];
  flags: Map<string, string | true>;
  rest: string[];
}

const VALUE_FLAGS = new Set(['dir', 'cwd', 'label', 'since', 'replay']);

/** 最小限の引数パーサ。`--` 以降は rest。--label は複数指定可 (値はカンマ連結でなく別管理)。 */
function parseArgs(argv: string[]): Parsed & { labels: string[] } {
  const positional: string[] = [];
  const flags = new Map<string, string | true>();
  const labels: string[] = [];
  let rest: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!;
    if (a === '--') {
      rest = argv.slice(i + 1);
      break;
    }
    if (a.startsWith('--')) {
      const name = a.slice(2);
      if (VALUE_FLAGS.has(name)) {
        const v = argv[++i];
        if (v === undefined) fail(`--${name} requires a value`);
        if (name === 'label') labels.push(v);
        else flags.set(name, v);
      } else flags.set(name, true);
    } else positional.push(a);
  }
  return { positional, flags, rest, labels };
}

function fail(msg: string): never {
  process.stderr.write(`misao-spike: ${msg}\n`);
  process.exit(2);
}

function defaultDir(): string {
  return process.env.MISAO_DIR ?? path.join(os.homedir(), 'workspace/misao/.run');
}

async function resolvePane(client: MisaoClient, query: string | undefined): Promise<string> {
  if (!query) fail('pane id required');
  const panes = await client.request<PaneInfo[]>('pane.list');
  const exact = panes.find((p) => p.paneId === query);
  if (exact) return exact.paneId;
  const q = query.toUpperCase().replace(/^P_/, '');
  const hits = panes.filter((p) => {
    const bare = p.paneId.slice(2).toUpperCase();
    return bare.startsWith(q) || bare.endsWith(q);
  });
  if (hits.length === 1) return hits[0]!.paneId;
  if (hits.length === 0) fail(`no pane matches "${query}"`);
  fail(`"${query}" is ambiguous: ${hits.map((h) => h.paneId).join(', ')}`);
}

function relTime(iso: string | null): string {
  if (!iso) return '-';
  const s = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h`;
}

function printTable(rows: string[][]): void {
  const widths = rows[0]!.map((_, c) => Math.max(...rows.map((r) => r[c]!.length)));
  for (const r of rows) console.log(r.map((cell, c) => (c === r.length - 1 ? cell : cell.padEnd(widths[c]!))).join('  '));
}

async function streamCommand(
  client: MisaoClient,
  method: string,
  params: Record<string, unknown>,
  follow: boolean,
  explicitSince: boolean,
  render: (n: { method: string; params: Record<string, unknown> & { seq: number } }) => void,
): Promise<void> {
  let head = -1;
  let last = 0;
  let finish: () => void = () => undefined;
  const finished = new Promise<void>((r) => (finish = r));
  client.onNotification((n) => {
    render(n);
    last = n.params.seq;
    if (!follow && head >= 0 && last >= head) finish();
  });
  const res = await client.request<{ gap: boolean; head: number }>(method, params);
  head = res.head;
  if (res.gap && explicitSince) process.stderr.write('[misao] warning: gap (requested history was already evicted)\n');
  if (!follow && (last >= head || (params.since as number) >= head)) finish();
  client.on('close', finish);
  await finished;
}

async function main(): Promise<number> {
  const args = parseArgs(process.argv.slice(2));
  const dir = path.resolve((args.flags.get('dir') as string | undefined) ?? defaultDir());
  const [cmd, ...pos] = args.positional;
  if (!cmd || args.flags.has('help')) {
    process.stdout.write(HELP);
    return cmd ? 0 : 2;
  }

  if (cmd === 'serve') {
    const daemon = new Daemon({ dir });
    await daemon.start();
    const stop = () => void daemon.shutdown().then(() => process.exit(0));
    process.once('SIGTERM', stop);
    process.once('SIGINT', stop);
    return -1; // 常駐
  }

  const client = await MisaoClient.connect(path.join(dir, 'misao.sock')).catch((e: Error) => {
    fail(`cannot connect to daemon in ${dir}: ${e.message}`);
  });
  try {
    return await runClientCommand(client, cmd, pos, args);
  } finally {
    client.close();
  }
}

async function runClientCommand(
  client: MisaoClient,
  cmd: string,
  pos: string[],
  args: Parsed & { labels: string[] },
): Promise<number> {
  const since = args.flags.get('since');
  const follow = args.flags.has('follow');
  switch (cmd) {
    case 'status':
      console.log(JSON.stringify(await client.request('server.info'), null, 2));
      return 0;
    case 'ls': {
      const panes = await client.request<PaneInfo[]>('pane.list');
      if (args.flags.has('json')) console.log(JSON.stringify(panes, null, 2));
      else
        printTable([
          ['STATE', 'PANE', 'TITLE', 'CMD', 'CWD', 'LAST'],
          ...panes.map((p) => [
            p.state === 'exited' ? `exited(${p.exitCode ?? `sig${p.signal}`})` : p.state,
            p.paneId,
            p.title || '-',
            p.cmd.join(' '),
            p.cwd,
            relTime(p.lastOutputAt),
          ]),
        ]);
      return 0;
    }
    case 'open': {
      if (args.rest.length === 0) fail('open requires: -- cmd...');
      const labels = Object.fromEntries(
        args.labels.map((l) => {
          const i = l.indexOf('=');
          return i < 0 ? [l, ''] : [l.slice(0, i), l.slice(i + 1)];
        }),
      );
      const cwd = args.flags.get('cwd');
      const { paneId } = await client.request<{ paneId: string }>('pane.open', {
        cmd: args.rest,
        cwd: typeof cwd === 'string' ? path.resolve(cwd) : undefined,
        cols: process.stdout.columns,
        rows: process.stdout.rows,
        labels,
      });
      console.log(paneId);
      return 0;
    }
    case 'send': {
      const paneId = await resolvePane(client, pos[0]);
      if (pos[1] === undefined) fail('send requires <text>');
      const data = pos[1] + (args.flags.has('enter') ? '\r' : '');
      await client.request('pane.write', { paneId, data, source: 'hub' });
      return 0;
    }
    case 'screen': {
      const paneId = await resolvePane(client, pos[0]);
      const s = await client.request<{ text: string; cursor: { x: number; y: number }; altScreen: boolean; title: string }>(
        'pane.screen',
        { paneId },
      );
      console.log(s.text.replace(/\n+$/, ''));
      process.stderr.write(`[cursor ${s.cursor.x},${s.cursor.y} alt=${s.altScreen} title=${JSON.stringify(s.title)}]\n`);
      return 0;
    }
    case 'tail': {
      const paneId = await resolvePane(client, pos[0]);
      const n = typeof since === 'string' ? Number(since) : 0;
      await streamCommand(client, 'pane.subscribe_lines', { paneId, since: n }, follow, typeof since === 'string', (m) => {
        if (m.method === 'pane.line') console.log(`${m.params.seq}\t${m.params.text as string}`);
      });
      return 0;
    }
    case 'events': {
      const n = typeof since === 'string' ? Number(since) : 0;
      await streamCommand(client, 'events.subscribe', { since: n }, follow, typeof since === 'string', (m) => {
        if (m.method === 'event') console.log(`${m.params.seq}\t${m.params.ts as string}\t${m.params.type as string}\t${JSON.stringify(m.params.data)}`);
      });
      return 0;
    }
    case 'attach': {
      const paneId = await resolvePane(client, pos[0]);
      const replay = (args.flags.get('replay') as string | undefined) ?? 'snapshot';
      if (!['raw', 'snapshot', 'none'].includes(replay)) fail('--replay must be raw|snapshot|none');
      return attach(client, { paneId, replay: replay as 'raw' | 'snapshot' | 'none', readonly: args.flags.has('readonly') });
    }
    case 'close': {
      const paneId = await resolvePane(client, pos[0]);
      await client.request('pane.close', { paneId });
      return 0;
    }
    default:
      fail(`unknown command: ${cmd}\n${HELP}`);
  }
}

main().then(
  (code) => {
    if (code >= 0) process.exit(code);
  },
  (e: Error) => {
    process.stderr.write(`misao-spike: ${e.message}\n`);
    process.exit(1);
  },
);
