import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MisaoClient } from '@misao/sdk';
import { TTY_RESET } from '../../src/attach/tty.js';
import { formatBanner, runSession } from '../../src/attach/session.js';
import type { SessionEnd } from '../../src/attach/session.js';
import type { KeyBindings } from '../../src/config/index.js';
import { createTestIo } from '../helpers/io.js';
import type { TestIo } from '../helpers/io.js';
import { openTestPane, startTestDaemon, waitFor } from '../helpers/daemon.js';
import type { TestDaemon } from '../helpers/daemon.js';
import { makePane } from '../helpers/pane.js';

const KEYS: KeyBindings = { prefix: 0x1e, detach: 0x64, next: 0x6e, prev: 0x70, list: 0x6c };
const PREFIX_D = Buffer.from([0x1e, 0x64]);
const READ_ONE = ['sh', '-c', 'echo ready-marker; read x; echo got:$x; sleep 60'];

interface Running {
  io: TestIo;
  end: Promise<SessionEnd>;
}

async function startSession(
  daemon: TestDaemon,
  cmd: string[],
  options: { readonly?: boolean; columns?: number; rows?: number } = {},
): Promise<Running & { paneId: string }> {
  const paneId = await openTestPane(daemon.client, cmd, { labels: { windowId: '806', name: 'セッション', task: '419' } });
  const io = createTestIo({ isTTY: true, columns: options.columns ?? 100, rows: options.rows ?? 30 });
  const pane = await daemon.client.request('pane.info', { paneId });
  const client = new MisaoClient({ socketPath: daemon.socketPath });
  await client.connect();
  const end = runSession({ client, io, pane, keys: KEYS, isReadonly: options.readonly ?? false, replay: 'snapshot' }).finally(() =>
    client.close(),
  );
  return { io, end, paneId };
}

function assertRestored(io: TestIo): void {
  assert.deepEqual(io.rawModes, [true, false]);
  assert.ok(io.out().endsWith(TTY_RESET), 'TTY_RESET を最後に書く');
}

test('formatBanner: 案内 1 行をキー設定から作る (--readonly で READONLY)', () => {
  const pane = makePane({ labels: { windowId: '806', name: 'misao 計画', task: '419' }, agentState: 'blocked', fgCommand: 'claude' });
  assert.equal(
    formatBanner(pane, KEYS, false, '/home/test'),
    '[misao] W-806 · misao 計画 · task #419 · claude · blocked · 抜ける: Ctrl-^ d · 次/前: Ctrl-^ n/p · 一覧: Ctrl-^ l',
  );
  const custom: KeyBindings = { ...KEYS, prefix: 0x01, detach: 0x71 };
  const readonly = formatBanner(makePane({ cmd: ['/bin/zsh'] }), custom, true, '/home/test');
  assert.match(readonly, /^\[misao\] \(未登録\) zsh · ~\/work · zsh · idle · READONLY · 抜ける: Ctrl-A q · /);
});

test('入出力をつなぎ、入力は source=terminal で送られ、サイズに追従し、prefix + d で抜ける', async () => {
  const daemon = await startTestDaemon();
  try {
    const { io, end, paneId } = await startSession(daemon, READ_ONE);
    assert.match(io.err(), /^\[misao\] W-806 · セッション · task #419 · sh · \w+ · 抜ける: Ctrl-\^ d/);
    await waitFor(() => io.out().includes('ready-marker'));
    assert.equal((await daemon.client.request('pane.info', { paneId })).clients.includes('cli-4242'), true);
    assert.deepEqual(
      [(await daemon.client.request('pane.info', { paneId })).cols, (await daemon.client.request('pane.info', { paneId })).rows],
      [100, 30],
    );

    io.stdin.write('abc\r');
    await waitFor(() => io.out().includes('got:abc'));
    io.setSize(120, 40);
    io.emitSignal('SIGWINCH');
    await waitFor(async () => (await daemon.client.request('pane.info', { paneId })).cols === 120);

    io.stdin.write(PREFIX_D);
    const result = await end;
    assert.deepEqual(result, { reason: 'action', action: 'detach', inputCount: 1 });
    assertRestored(io);
    await waitFor(async () => !(await daemon.client.request('pane.info', { paneId })).clients.includes('cli-4242'));
  } finally {
    await daemon.stop();
  }
});

test('端末がサイズ 0 を報告しても attach でき、ペインのサイズは変えない', async () => {
  const daemon = await startTestDaemon();
  try {
    const { io, end, paneId } = await startSession(daemon, READ_ONE, { columns: 0, rows: 0 });
    const before = await daemon.client.request('pane.info', { paneId });
    await waitFor(() => io.out().includes('ready-marker'));
    io.emitSignal('SIGWINCH');
    io.stdin.write('abc\r');
    await waitFor(() => io.out().includes('got:abc'));
    const after = await daemon.client.request('pane.info', { paneId });
    assert.deepEqual([after.cols, after.rows], [before.cols, before.rows]);
    io.stdin.write(PREFIX_D);
    assert.deepEqual(await end, { reason: 'action', action: 'detach', inputCount: 1 });
    assertRestored(io);
  } finally {
    await daemon.stop();
  }
});

test('prefix 2 回は prefix の 1 バイトを送り、その他のキーは捨てる。入力回数は pane.write の回数', async () => {
  const daemon = await startTestDaemon();
  try {
    const { io, end } = await startSession(daemon, ['sh', '-c', 'cat -v; sleep 60']);
    await waitFor(() => io.out().length > 0);
    io.stdin.write(Buffer.from([0x1e, 0x1e]));
    await waitFor(() => io.out().includes('^^'));
    io.stdin.write(Buffer.from([0x1e, 0x78, 0x1e, 0x6e]));
    assert.deepEqual(await end, { reason: 'action', action: 'next', inputCount: 1 });
    assertRestored(io);
  } finally {
    await daemon.stop();
  }
});

test('--readonly は入力を送らず resize もしない (prefix のアクションは効く)', async () => {
  const daemon = await startTestDaemon();
  try {
    const { io, end, paneId } = await startSession(daemon, READ_ONE, { readonly: true });
    assert.match(io.err(), /READONLY/);
    await waitFor(() => io.out().includes('ready-marker'));
    io.stdin.write('typed\r');
    io.setSize(150, 50);
    io.emitSignal('SIGWINCH');
    io.stdin.write(PREFIX_D);
    assert.deepEqual(await end, { reason: 'action', action: 'detach', inputCount: 0 });
    const info = await daemon.client.request('pane.info', { paneId });
    assert.deepEqual([info.cols, info.rows], [80, 24]);
    assert.doesNotMatch((await daemon.client.request('pane.screen', { paneId })).text, /got:/);
    assertRestored(io);
  } finally {
    await daemon.stop();
  }
});

test('ペインが終了したら exited で戻り、端末を戻す', async () => {
  const daemon = await startTestDaemon();
  try {
    const { io, end } = await startSession(daemon, ['sh', '-c', 'echo bye; sleep 0.5; exit 3']);
    assert.deepEqual(await end, { reason: 'exited', exitCode: 3, signal: null, inputCount: 0 });
    assertRestored(io);
  } finally {
    await daemon.stop();
  }
});

test('デーモンとの接続が切れたら disconnected で戻り、端末を戻す', async () => {
  const daemon = await startTestDaemon();
  const { io, end } = await startSession(daemon, READ_ONE);
  await waitFor(() => io.out().includes('ready-marker'));
  await daemon.stop();
  assert.deepEqual(await end, { reason: 'disconnected', inputCount: 0 });
  assertRestored(io);
});

test('SIGTERM では signal で戻り、端末を戻す', async () => {
  const daemon = await startTestDaemon();
  try {
    const { io, end } = await startSession(daemon, READ_ONE);
    await waitFor(() => io.out().includes('ready-marker'));
    io.emitSignal('SIGTERM');
    assert.deepEqual(await end, { reason: 'signal', inputCount: 0 });
    assertRestored(io);
  } finally {
    await daemon.stop();
  }
});

test('attach が拒否されて例外になっても端末を戻す', async () => {
  const daemon = await startTestDaemon();
  try {
    const paneId = await openTestPane(daemon.client, ['sh', '-c', 'sleep 60']);
    const pane = await daemon.client.request('pane.info', { paneId });
    await daemon.client.request('pane.close', { paneId });
    const io = createTestIo({ isTTY: true, columns: 100, rows: 30 });
    const client = new MisaoClient({ socketPath: daemon.socketPath });
    await client.connect();
    await assert.rejects(runSession({ client, io, pane, keys: KEYS, isReadonly: false, replay: 'snapshot' }), /not found/i);
    client.close();
    assertRestored(io);
  } finally {
    await daemon.stop();
  }
});

test('端末でなければ raw mode に入る前に usage エラー', async () => {
  const daemon = await startTestDaemon();
  try {
    const pane = makePane();
    const io = createTestIo({ columns: 100, rows: 30 });
    const client = new MisaoClient({ socketPath: daemon.socketPath });
    await client.connect();
    await assert.rejects(runSession({ client, io, pane, keys: KEYS, isReadonly: false, replay: 'snapshot' }), /TTY/);
    client.close();
    assert.deepEqual(io.rawModes, []);
  } finally {
    await daemon.stop();
  }
});
