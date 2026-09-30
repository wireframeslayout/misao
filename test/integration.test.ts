import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as net from 'node:net';
import * as path from 'node:path';
import xterm from '@xterm/headless';
import { viewportText } from '../src/daemon/screen.js';
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
    await client.request('pane.attach', { paneId, clientId: 'c1', replay: 'raw' });
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
    delete process.env.TMUX;
    client.close();
    await daemon.shutdown();
  }
});

async function withDaemon(fn: (daemon: Daemon, client: MisaoClient) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  const daemon = new Daemon({ dir, log: () => undefined });
  await daemon.start();
  const client = await MisaoClient.connect(daemon.socketPath);
  try {
    await fn(daemon, client);
  } finally {
    client.close();
    await daemon.shutdown();
  }
}

test('snapshot attach: 出力が流れ続ける pane で snapshot + live == pane.screen (境界の重複なし)', async () => {
  await withDaemon(async (daemon, client) => {
    const { paneId } = await client.request<{ paneId: string }>('pane.open', {
      cmd: ['sh', '-c', 'i=0; while [ $i -lt 6000 ]; do printf "\\033[3$((i%7+1))mline $i\\033[0m\\r\\n"; i=$((i+1)); done; echo FINISHED; sleep 5'],
    });
    await new Promise((r) => setTimeout(r, 30));
    const c2 = await MisaoClient.connect(daemon.socketPath);
    const term = new xterm.Terminal({ cols: 80, rows: 24, scrollback: 100, allowProposedApi: true });
    const feed = (b: Buffer) => new Promise<void>((r) => term.write(b, r));
    const parts: Buffer[] = [];
    let text = '';
    c2.onNotification((n) => {
      if (n.method !== 'pane.output') return;
      const b = Buffer.from(n.params.dataB64 as string, 'base64');
      parts.push(b);
      text = (text + b.toString()).slice(-200);
    });
    const res = await c2.request<{ headSeq: number }>('pane.attach', { paneId, clientId: 'snap', replay: 'snapshot' });
    assert.ok(res.headSeq >= 0);
    await waitFor(() => text.includes('FINISHED'));
    for (const p of parts) await feed(p);
    const expected = await client.request<{ text: string }>('pane.screen', { paneId });
    assert.equal(viewportText(term), expected.text);
    c2.close();
    await client.request('pane.close', { paneId });
  });
});

test('同じ (stream, pane) の再購読は置き換えで、since=lastSeq の再購読でも seq は連続・重複なし', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await client.request<{ paneId: string }>('pane.open', {
      cmd: ['sh', '-c', 'echo a; echo b; echo c; sleep 1; echo d; echo e; sleep 3'],
    });
    const seqs: number[] = [];
    client.onNotification((n) => n.method === 'pane.line' && seqs.push(n.params.seq));
    await client.request('pane.subscribe_lines', { paneId, since: 0 });
    await waitFor(() => seqs.length >= 3);
    await client.request('pane.subscribe_lines', { paneId, since: seqs[seqs.length - 1] });
    await client.request('pane.subscribe_lines', { paneId, since: seqs[seqs.length - 1] });
    await waitFor(() => seqs.length >= 5);
    await new Promise((r) => setTimeout(r, 200));
    assert.deepEqual(seqs, [1, 2, 3, 4, 5]);
    await client.request('pane.close', { paneId });
  });
});

test('サイズは最後に操作したクライアントが優先される', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await client.request<{ paneId: string }>('pane.open', { cmd: ['sh', '-c', 'cat'] });
    const info = async () => (await client.request<PaneInfo[]>('pane.list'))[0]!;
    const c2 = await MisaoClient.connect(_daemon.socketPath);
    await client.request('pane.attach', { paneId, clientId: 'A', replay: 'none', cols: 100, rows: 30 });
    await c2.request('pane.attach', { paneId, clientId: 'B', replay: 'none', cols: 60, rows: 20 });
    let i = await info();
    assert.deepEqual([i.cols, i.rows, i.sizeOwner], [60, 20, 'B']);
    await client.request('pane.write', { paneId, data: 'x', source: 'terminal', clientId: 'A' });
    i = await info();
    assert.deepEqual([i.cols, i.rows, i.sizeOwner], [100, 30, 'A']);
    await client.request('pane.write', { paneId, data: 'y', source: 'hub' }); // clientId なしは変更しない
    assert.equal((await info()).sizeOwner, 'A');
    await c2.request('pane.resize', { paneId, cols: 70, rows: 25, clientId: 'B' });
    assert.deepEqual([(await info()).cols, (await info()).sizeOwner], [70, 'B']);
    // 所有者 B が detach したら、残る A のサイズが適用される
    await c2.request('pane.detach', { paneId });
    const after = await info();
    assert.deepEqual([after.cols, after.rows, after.sizeOwner], [100, 30, 'A']);
    c2.close();
    await client.request('pane.close', { paneId });
  });
});

test('raw attach の結果に oldest / truncated が載る', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await client.request<{ paneId: string }>('pane.open', { cmd: ['sh', '-c', 'echo hi; sleep 3'] });
    await new Promise((r) => setTimeout(r, 300));
    const res = await client.request<{ headSeq: number; oldest: number; truncated: boolean }>('pane.attach', {
      paneId,
      clientId: 'r',
      replay: 'raw',
    });
    assert.equal(res.oldest, 1);
    assert.equal(res.truncated, false);
    await client.request('pane.close', { paneId });
  });
});

test('改行なしで終了する最終行も行ストリームに流れる', async () => {
  await withDaemon(async (_daemon, client) => {
    const lines: string[] = [];
    client.onNotification((n) => n.method === 'pane.line' && lines.push(n.params.text as string));
    const { paneId } = await client.request<{ paneId: string }>('pane.open', {
      cmd: ['sh', '-c', 'sleep 0.3; printf AZITO_DONE_y_1'],
    });
    await client.request('pane.subscribe_lines', { paneId, since: 0 });
    await waitFor(() => lines.includes('AZITO_DONE_y_1'));
    await client.request('pane.close', { paneId });
  });
});

test('オブジェクトでない JSON を受けても daemon は落ちず Invalid Request を返す', async () => {
  await withDaemon(async (daemon, client) => {
    const raw = net.connect(daemon.socketPath);
    await new Promise((r) => raw.once('connect', r));
    const got: string[] = [];
    raw.on('data', (d) => got.push(d.toString()));
    raw.write('1\n[]\nnull\n"x"\n{not json\n');
    await waitFor(() => got.join('').split('\n').filter(Boolean).length >= 5);
    const replies = got.join('').split('\n').filter(Boolean).map((l) => JSON.parse(l));
    assert.deepEqual(replies.map((r) => r.error.code), [-32600, -32600, -32600, -32600, -32700]);
    raw.destroy();
    assert.equal((await client.request<{ pid: number }>('server.info')).pid, process.pid);
  });
});
