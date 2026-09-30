import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from '../src/daemon/Daemon.js';
import { MisaoClient } from '../src/client/MisaoClient.js';
import type { PaneInfo } from '../src/daemon/Pane.js';

function waitFor(pred: () => boolean, ms = 5000): Promise<void> {
  return new Promise((resolve, reject) => {
    const t0 = Date.now();
    const tick = () => (pred() ? resolve() : Date.now() - t0 > ms ? reject(new Error('timeout')) : setTimeout(tick, 20));
    tick();
  });
}

test('daemon: pane.open → subscribe_lines でマーカーを観測 → 停止', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  const daemon = new Daemon({ dir, log: () => undefined });
  await daemon.start();
  const client = await MisaoClient.connect(daemon.socketPath);
  try {
    assert.equal((fs.statSync(daemon.socketPath).mode & 0o777), 0o600);
    assert.equal(fs.readFileSync(path.join(dir, 'daemon.pid'), 'utf8'), String(process.pid));

    const lines: Array<{ seq: number; text: string }> = [];
    const events: string[] = [];
    client.onNotification((n) => {
      if (n.method === 'pane.line') lines.push({ seq: n.params.seq, text: n.params.text as string });
      if (n.method === 'event') events.push(n.params.type as string);
    });
    await client.request('events.subscribe', { since: 0 });

    const { paneId } = await client.request<{ paneId: string }>('pane.open', {
      cmd: ['sh', '-c', 'printf "hello\\r\\nAZITO_DONE_x_1\\r\\n"; sleep 1'],
      labels: { role: 'test' },
    });
    assert.match(paneId, /^p_[0-9A-Z]{26}$/);
    const res = await client.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since: 0 });
    assert.equal(res.gap, false);

    await waitFor(() => lines.some((l) => l.text === 'AZITO_DONE_x_1'));
    assert.ok(lines.some((l) => l.text === 'hello'));
    assert.deepEqual(lines.map((l) => l.seq), [...lines.map((l) => l.seq)].sort((a, b) => a - b));

    // 再購読 (since=0) でリングから再生され、seq が重複せず揃う
    const replayed: number[] = [];
    client.onNotification((n) => n.method === 'pane.line' && replayed.push(n.params.seq));
    await client.request('pane.subscribe_lines', { paneId, since: 0 });
    await waitFor(() => replayed.length >= 2);

    await waitFor(() => events.includes('pane.exited'));
    const list = await client.request<PaneInfo[]>('pane.list');
    assert.equal(list.length, 1);
    assert.equal(list[0]!.state, 'exited');
    assert.equal(list[0]!.exitCode, 0);
    assert.deepEqual(list[0]!.labels, { role: 'test' });
    assert.ok(events.includes('daemon.started') && events.includes('pane.opened'));

    await client.request('pane.close', { paneId });
    assert.equal((await client.request<PaneInfo[]>('pane.list')).length, 0);
  } finally {
    client.close();
    await daemon.shutdown();
  }
  assert.equal(fs.existsSync(daemon.socketPath), false);
});

test('daemon: write / screen / snapshot attach / env 除去', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  const daemon = new Daemon({ dir, log: () => undefined });
  await daemon.start();
  const client = await MisaoClient.connect(daemon.socketPath);
  try {
    process.env.TMUX = 'leaked-from-parent';
    const chunks: Buffer[] = [];
    client.onNotification((n) => {
      if (n.method === 'pane.output') chunks.push(Buffer.from(n.params.dataB64 as string, 'base64'));
    });
    const { paneId } = await client.request<{ paneId: string }>('pane.open', {
      cmd: ['sh', '-c', 'echo "TMUX=[$TMUX] PANE=[$MISAO_PANE_ID] TERM=$TERM"; printf "\\033[31mred\\033[0m\\n"; exec cat'],
    });
    await client.request('pane.attach', { paneId, clientId: 'c1', replay: 'none' });
    await waitFor(() => Buffer.concat(chunks).toString().includes('red'));
    const screen = await client.request<{ text: string; altScreen: boolean }>('pane.screen', { paneId });
    assert.match(screen.text, new RegExp(`PANE=\\[${paneId}\\]`));
    assert.match(screen.text, /TMUX=\[\] /);
    assert.match(screen.text, /TERM=xterm-256color/);
    assert.equal(screen.altScreen, false);

    await client.request('pane.write', { paneId, data: 'typed\r', source: 'terminal' });
    await waitFor(() => Buffer.concat(chunks).toString().includes('typed'));

    // 別クライアントで snapshot attach: 赤 SGR と本文が再現される
    const c2 = await MisaoClient.connect(daemon.socketPath);
    const snap: Buffer[] = [];
    c2.onNotification((n) => n.method === 'pane.output' && snap.push(Buffer.from(n.params.dataB64 as string, 'base64')));
    await c2.request('pane.attach', { paneId, clientId: 'c2', replay: 'snapshot' });
    await waitFor(() => snap.length > 0);
    const text = Buffer.concat(snap).toString();
    assert.match(text, /\x1b\[0;31mred/);
    assert.match(text, /typed/);
    const list = await client.request<PaneInfo[]>('pane.list');
    assert.deepEqual(list[0]!.clients.sort(), ['c1', 'c2']);
    c2.close();
    await client.request('pane.close', { paneId });
  } finally {
    client.close();
    await daemon.shutdown();
  }
});
