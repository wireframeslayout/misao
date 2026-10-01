// テスト用の偽 hub。events と全 pane の lines を購読し、受信した内容を NDJSON でログへ追記する。
// 位置 ({seq, epoch}) を state ファイルに保存し、再起動時はそこから再開する。
import * as fs from 'node:fs';
import { MisaoClient } from '@misao/sdk';
import type { StreamPosition } from '@misao/sdk';

interface HubState {
  events?: StreamPosition;
  lines: Record<string, StreamPosition>;
}

function readFlags(argv: readonly string[]): { socket: string; stateFile: string; logFile: string } {
  const flag = (name: string): string => {
    const i = argv.indexOf(`--${name}`);
    const value = i >= 0 ? argv[i + 1] : undefined;
    if (value === undefined) {
      process.stderr.write('usage: fake-hub --socket PATH --state FILE --log FILE\n');
      process.exit(2);
    }
    return value;
  };
  return { socket: flag('socket'), stateFile: flag('state'), logFile: flag('log') };
}

function loadState(stateFile: string): HubState {
  if (!fs.existsSync(stateFile)) return { lines: {} };
  return JSON.parse(fs.readFileSync(stateFile, 'utf8')) as HubState;
}

const { socket, stateFile, logFile } = readFlags(process.argv.slice(2));
const state = loadState(stateFile);

/** ログへ追記してから state を保存する。落ちても重複で済み、取りこぼしは出ない。 */
function record(entry: Record<string, unknown>, save?: () => void): void {
  fs.appendFileSync(logFile, JSON.stringify(entry) + '\n');
  if (save === undefined) return;
  save();
  fs.writeFileSync(`${stateFile}.tmp`, JSON.stringify(state));
  fs.renameSync(`${stateFile}.tmp`, stateFile);
}

const client = new MisaoClient({ socketPath: socket });
await client.connect();
let epoch = (await client.request('server.info', {})).epoch;

client.onGap(({ stream, reason }) => {
  record({ kind: 'gap', stream, reason });
  // epoch が変わったら以後の位置は新しい epoch のもの。取得するまでの保存は古い epoch のままで、再起動時は最古から再生される (安全側)。
  if (reason === 'epoch') void client.request('server.info', {}).then((info) => (epoch = info.epoch));
});
client.onSubscriptionError(({ stream, error }) => record({ kind: 'subscription-error', stream, message: error.message }));

const watched = new Set<string>();

async function watchLines(paneId: string): Promise<void> {
  if (watched.has(paneId)) return;
  watched.add(paneId);
  const position = state.lines[paneId] ?? { seq: 0, epoch };
  await client.subscribeLines(
    paneId,
    (line) => record({ kind: 'line', ...line }, () => (state.lines[paneId] = { seq: line.seq, epoch })),
    { since: position.seq, epoch: position.epoch },
  );
  record({ kind: 'subscribed', stream: 'lines', paneId, since: position.seq });
}

const eventsFrom = state.events ?? { seq: 0, epoch };
await client.subscribeEvents(
  (event) => {
    record({ kind: 'event', ...event }, () => (state.events = { seq: event.seq, epoch }));
    if (event.type === 'pane.opened' && event.paneId !== undefined) {
      void watchLines(event.paneId).catch((e: unknown) => record({ kind: 'watch-error', message: String(e) }));
    }
  },
  { since: eventsFrom.seq, epoch: eventsFrom.epoch },
);
record({ kind: 'subscribed', stream: 'events', since: eventsFrom.seq });

for (const pane of await client.request('pane.list', {})) await watchLines(pane.paneId);
