import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from '../src/daemon.js';
import type { DaemonOptions } from '../src/daemon.js';
import { RpcClient } from './helpers/rpc-client.js';
import { startDaemon, waitFor } from './helpers/daemon.js';

async function withLimitedDaemon(
  overrides: Partial<DaemonOptions>,
  fn: (daemon: Daemon, client: RpcClient) => Promise<void>,
): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-limits-'));
  const daemon = await startDaemon(dir, overrides);
  const client = await RpcClient.connect(daemon.socketPath);
  try {
    await fn(daemon, client);
  } finally {
    client.close();
    await daemon.shutdown();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const SEVEN_LINES = 'for i in 1 2 3 4 5 6 7; do echo "line-$i-xxxxxxxxxxxxxxxx"; sleep 0.1; done; echo AZITO_DONE; sleep 5';

async function openAndWaitDone(client: RpcClient, cmd: string[], extra: Record<string, unknown> = {}): Promise<string> {
  const lines: string[] = [];
  client.onNotification((n) => n.method === 'pane.line' && lines.push(n.params.text as string));
  const { paneId } = await client.request<{ paneId: string }>('pane.open', { cmd, ...extra });
  await client.request('pane.subscribe_lines', { paneId, since: 0 });
  await waitFor(() => lines.includes('AZITO_DONE'));
  return paneId;
}

test('rings.linesBytes が小さいと、行の購読は gap で最古が 1 より後になる', async () => {
  await withLimitedDaemon({ rings: { linesBytes: 40 } }, async (_d, client) => {
    const paneId = await openAndWaitDone(client, ['sh', '-c', SEVEN_LINES]);
    const seqs: number[] = [];
    client.onNotification((n) => n.method === 'pane.line' && seqs.push(n.params.seq as number));
    const res = await client.request<{ gap: boolean }>('pane.subscribe_lines', { paneId, since: 0 });
    assert.equal(res.gap, true);
    await waitFor(() => seqs.length > 0);
    assert.ok(seqs[0]! > 1, `first replayed seq: ${seqs[0]}`);
    await client.request('pane.close', { paneId });
  });
});

test('rings.rawBytes が小さいと、raw replay は truncated になる', async () => {
  await withLimitedDaemon({ rings: { rawBytes: 40 } }, async (_d, client) => {
    const paneId = await openAndWaitDone(client, ['sh', '-c', SEVEN_LINES]);
    const res = await client.request<{ oldest: number; truncated: boolean }>('pane.attach', {
      paneId,
      clientId: 'r',
      replay: 'raw',
    });
    assert.equal(res.truncated, true);
    assert.ok(res.oldest > 1, `oldest: ${res.oldest}`);
    await client.request('pane.close', { paneId });
  });
});

test('rings.events が小さいと、events.subscribe since=0 は gap になる', async () => {
  await withLimitedDaemon({ rings: { events: 2 } }, async (_d, client) => {
    const { paneId } = await client.request<{ paneId: string }>('pane.open', { cmd: ['sh', '-c', 'sleep 5'] });
    const res = await client.request<{ gap: boolean }>('events.subscribe', { since: 0 });
    assert.equal(res.gap, true);
    await client.request('pane.close', { paneId });
  });
});

test('既定の上限では、同じ操作で gap にならない', async () => {
  await withLimitedDaemon({}, async (_d, client) => {
    const paneId = await openAndWaitDone(client, ['sh', '-c', SEVEN_LINES]);
    const lines = await client.request<{ gap: boolean }>('pane.subscribe_lines', { paneId, since: 0 });
    const events = await client.request<{ gap: boolean }>('events.subscribe', { since: 0 });
    assert.equal(lines.gap, false);
    assert.equal(events.gap, false);
    await client.request('pane.close', { paneId });
  });
});

/** headless 端末のバッファ長 (画面の行数 + スクロールバック)。公開 API では読めないので内部を覗く。 */
async function bufferLength(daemon: Daemon, client: RpcClient, paneId: string): Promise<number> {
  await client.request('pane.screen', { paneId }); // 書き込みの反映を待つ
  const pane = daemon['registry'].livePanes().find((p) => p.id === paneId);
  assert.ok(pane);
  return pane['term'].buffer.active.length;
}

test('scrollback は headless 端末に渡り、バッファ長は rows + scrollback で頭打ちになる', async () => {
  const cmd = ['sh', '-c', 'i=0; while [ $i -lt 100 ]; do echo row$i; i=$((i+1)); done; echo AZITO_DONE; sleep 5'];
  await withLimitedDaemon({ scrollback: 10 }, async (daemon, client) => {
    const paneId = await openAndWaitDone(client, cmd, { rows: 5, cols: 40 });
    assert.equal(await bufferLength(daemon, client, paneId), 5 + 10);
    await client.request('pane.close', { paneId });
  });
  await withLimitedDaemon({}, async (daemon, client) => {
    const paneId = await openAndWaitDone(client, cmd, { rows: 5, cols: 40 });
    assert.ok((await bufferLength(daemon, client, paneId)) > 5 + 10, '既定の 5000 行なら 100 行は頭打ちにならない');
    await client.request('pane.close', { paneId });
  });
});

test('logLevel=error では listening (info) が出ず、既定では出る', async () => {
  for (const [logLevel, expected] of [['error', false], [undefined, true]] as const) {
    const logs: string[] = [];
    await withLimitedDaemon({ log: (m) => logs.push(m), ...(logLevel ? { logLevel } : {}) }, async () => undefined);
    assert.equal(logs.some((m) => m.startsWith('listening on')), expected, `${logLevel}: ${logs.join('|')}`);
  }
});

test('0・負数・小数の上限は constructor で RangeError', () => {
  const base = { version: '0.0.0-test', socketPath: '/tmp/x.sock', pidPath: '/tmp/x.pid', statePath: '/tmp/x.json' };
  const bad: Array<Partial<DaemonOptions>> = [
    { scrollback: 0 },
    { rings: { rawBytes: 0 } },
    { rings: { linesBytes: -1 } },
    { rings: { events: 1.5 } },
  ];
  for (const o of bad) assert.throws(() => new Daemon({ ...base, ...o }), RangeError, JSON.stringify(o));
});
