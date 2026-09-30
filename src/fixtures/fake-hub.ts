// テスト用の偽 hub。events と 1 pane の lines を購読し、受信した通知を NDJSON でログへ追記する。
// lastSeq を state ファイルに保存し、再接続時は since から再開する。
import * as fs from 'node:fs';
import * as path from 'node:path';
import { MisaoClient } from '../client/MisaoClient.js';

const argv = process.argv.slice(2);
function flag(name: string): string | undefined {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv[i + 1] : undefined;
}
const dir = flag('dir') ?? process.env.MISAO_DIR;
const paneQuery = flag('pane');
const stateFile = flag('state');
const logFile = flag('log');
// --tag は argv に載せて ps / pgrep で識別するためのもの (値は使わない)
if (!dir || !stateFile || !logFile) {
  process.stderr.write('usage: fake-hub --dir DIR --state FILE --log FILE [--pane ID] --tag TAG\n');
  process.exit(2);
}

interface State {
  epoch?: string;
  events: number;
  lines: Record<string, number>;
}

function loadState(): State {
  try {
    return JSON.parse(fs.readFileSync(stateFile!, 'utf8')) as State;
  } catch {
    return { events: 0, lines: {} };
  }
}
const state = loadState();
function saveState(): void {
  const tmp = `${stateFile}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(state));
  fs.renameSync(tmp, stateFile!);
}

function log(entry: unknown): void {
  fs.appendFileSync(logFile!, JSON.stringify(entry) + '\n');
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const watched = new Set<string>();

/** デーモン再起動で epoch が変わったら seq が巻き戻るので since を捨てる。 */
function resetIfNewEpoch(epoch: string): void {
  if (state.epoch === epoch) return;
  if (state.epoch !== undefined) log({ kind: 'epoch-changed', from: state.epoch, to: epoch });
  state.epoch = epoch;
  state.events = 0;
  state.lines = {};
  saveState();
}

async function subscribeLines(client: MisaoClient, paneId: string): Promise<void> {
  const since = state.lines[paneId] ?? 0;
  const res = await client.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since });
  log({ kind: 'subscribed', stream: 'lines', paneId, since, ...res });
}

async function session(): Promise<void> {
  const client = await MisaoClient.connect(path.join(dir!, 'misao.sock'));
  const closed = new Promise<void>((r) => client.on('close', r));
  client.onNotification((n) => {
    log(n);
    if (n.method === 'event') {
      state.events = n.params.seq;
      if (n.params.type === 'pane.opened' && !paneQuery) {
        const id = (n.params.data as { paneId: string }).paneId;
        if (!watched.has(id)) {
          watched.add(id);
          void subscribeLines(client, id).catch(() => undefined);
        }
      }
    } else if (n.method === 'pane.line') {
      state.lines[n.params.paneId as string] = n.params.seq;
    }
    saveState();
  });
  const info = await client.request<{ epoch: string }>('server.info');
  resetIfNewEpoch(info.epoch);
  const ev = await client.request<{ gap: boolean; head: number }>('events.subscribe', { since: state.events });
  log({ kind: 'subscribed', stream: 'events', since: state.events, ...ev });
  // 既存 pane (再接続時を含む) の lines を購読
  const panes = await client.request<Array<{ paneId: string }>>('pane.list');
  watched.clear();
  for (const p of panes) {
    if (paneQuery && p.paneId !== paneQuery) continue;
    watched.add(p.paneId);
    await subscribeLines(client, p.paneId);
  }
  await closed;
}

let backoff = 200;
for (;;) {
  try {
    await session();
    backoff = 200;
    log({ kind: 'disconnected' });
  } catch (e) {
    log({ kind: 'connect-error', message: (e as Error).message });
  }
  await sleep(backoff);
  backoff = Math.min(backoff * 2, 5000);
}
