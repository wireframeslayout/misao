import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MisaoClient } from '@misao/sdk';
import { runAttachLoop } from '../../src/attach/loop.js';
import type { KeyBindings } from '../../src/config/index.js';
import { run } from '../../src/run.js';
import { createTestIo } from '../helpers/io.js';
import type { TestIo } from '../helpers/io.js';
import { openTestPane, startTestDaemon, waitFor } from '../helpers/daemon.js';
import type { TestDaemon } from '../helpers/daemon.js';

const KEYS: KeyBindings = { prefix: 0x1e, detach: 0x64, next: 0x6e, prev: 0x70, list: 0x6c };
const key = (k: string): Buffer => Buffer.from([0x1e, k.charCodeAt(0)]);
const ECHO = (name: string): string[] => ['sh', '-c', `echo marker-${name}; sleep 60`];

async function startLoop(daemon: TestDaemon, initial: string, options: { readonly?: boolean } = {}): Promise<{ io: TestIo; end: Promise<number>; client: MisaoClient }> {
  const io = createTestIo({ isTTY: true, columns: 100, rows: 30, shell: '/bin/sh', cwd: daemon.dir, homeDir: daemon.dir });
  const client = new MisaoClient({ socketPath: daemon.socketPath });
  await client.connect();
  const pane = await client.request('pane.info', { paneId: initial });
  const end = runAttachLoop({ client, io, keys: KEYS, initial: pane, isReadonly: options.readonly ?? false, replay: 'snapshot' }).finally(() =>
    client.close(),
  );
  return { io, end, client };
}

/** stderr のバナー (1 セッションにつき 1 行) の数。一覧の入力待ち (`> `) と同じ行になることがある。 */
const bannerCount = (io: TestIo): number => io.err().split('\n').filter((l) => l.includes('[misao] ') && l.includes('抜ける:')).length;

test('n / p は一覧 (ls と同じ並び) の隣へ循環して移動し、d で一覧に戻って q で終了 0', async () => {
  const daemon = await startTestDaemon();
  try {
    // idle のまま並ぶ 2 つ + 新しい出力を持つもの。lastOutputAt の新しい順で並ぶ。
    const first = await openTestPane(daemon.client, ECHO('one'), { labels: { name: 'loop-one' } });
    await waitFor(async () => (await daemon.client.request('pane.screen', { paneId: first })).text.includes('marker-one'));
    const second = await openTestPane(daemon.client, ECHO('two'), { labels: { name: 'loop-two' } });
    await waitFor(async () => (await daemon.client.request('pane.screen', { paneId: second })).text.includes('marker-two'));

    const { io, end } = await startLoop(daemon, first);
    await waitFor(() => io.out().includes('marker-one'));
    io.stdin.write(key('n'));
    await waitFor(() => bannerCount(io) === 2);
    assert.match(io.err().split('\n').filter((l) => l.includes('抜ける:')).at(-1)!, /loop-two|loop-one/);
    const after = io.out();
    io.stdin.write(key('p'));
    await waitFor(() => bannerCount(io) === 3);
    assert.ok(io.out().length > after.length);

    io.stdin.write(key('d'));
    await waitFor(() => io.out().includes('番号で入る'));
    assert.match(io.out(), /から抜けました（入力 0 回を記録。内容は保存していません）/);
    io.stdin.write('q\n');
    assert.equal(await end, 0);
    assert.equal(io.rawModes.filter(Boolean).length, io.rawModes.filter((m) => !m).length, '入った回数だけ戻している');
    await daemon.client.request('pane.close', { paneId: first });
    await daemon.client.request('pane.close', { paneId: second });
  } finally {
    await daemon.stop();
  }
});

test('l は「抜けました」を出さずに一覧へ。番号で別のペインへ入れる', async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ECHO('a'), { labels: { name: 'pick-a' } });
    const b = await openTestPane(daemon.client, ECHO('b'), { labels: { name: 'pick-b' } });
    const { io, end } = await startLoop(daemon, a);
    await waitFor(() => bannerCount(io) === 1);
    io.stdin.write(key('l'));
    await waitFor(() => io.out().includes('番号で入る'));
    assert.doesNotMatch(io.out(), /から抜けました/);
    const lines = io.out().split('\n');
    const row = lines.find((l) => l.includes('pick-b'))!;
    io.stdin.write(`${/^\s*(\d+)/.exec(row)![1]}\n`);
    await waitFor(() => bannerCount(io) === 2);
    assert.match(io.err().split('\n').filter((l) => l.includes('抜ける:')).at(-1)!, /pick-b/);
    io.stdin.write(key('d'));
    await waitFor(() => (io.out().match(/番号で入る/g)?.length ?? 0) === 2);
    io.stdin.write('q\n');
    assert.equal(await end, 0);
    await daemon.client.request('pane.close', { paneId: a });
    await daemon.client.request('pane.close', { paneId: b });
  } finally {
    await daemon.stop();
  }
});

test('一覧の n は origin=terminal の新しいシェルを作って入る', async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ECHO('base'), { labels: { name: 'base' } });
    const { io, end } = await startLoop(daemon, a);
    await waitFor(() => bannerCount(io) === 1);
    io.stdin.write(key('l'));
    await waitFor(() => io.out().includes('番号で入る'));
    io.stdin.write('n\n');
    await waitFor(() => bannerCount(io) === 2);
    const panes = await daemon.client.request('pane.list', {});
    const created = panes.find((p) => p.paneId !== a)!;
    assert.deepEqual(created.cmd, ['/bin/sh', '-l']);
    assert.equal(created.labels.origin, 'terminal');
    io.stdin.write(key('d'));
    await waitFor(() => (io.out().match(/番号で入る/g)?.length ?? 0) === 2);
    io.stdin.write('q\n');
    assert.equal(await end, 0);
    for (const p of panes) await daemon.client.request('pane.close', { paneId: p.paneId });
  } finally {
    await daemon.stop();
  }
});

test('ペインが終了したら一覧に戻り、入れるペインが無ければ 0 で終わる', async () => {
  const daemon = await startTestDaemon();
  try {
    const only = await openTestPane(daemon.client, ['sh', '-c', 'sleep 0.5; exit 2'], { labels: { windowId: '77' } });
    const { io, end } = await startLoop(daemon, only);
    assert.equal(await end, 0);
    assert.match(io.out(), /W-77 が終了しました（終了コード 2）/);
    assert.deepEqual(io.rawModes, [true, false]);
  } finally {
    await daemon.stop();
  }
});

test('終了済みのペインには入らず 1 (ペインが終わっている)', async () => {
  const daemon = await startTestDaemon();
  try {
    const gone = await openTestPane(daemon.client, ['sh', '-c', 'exit 0']);
    await waitFor(async () => (await daemon.client.request('pane.info', { paneId: gone })).processState === 'exited');
    const io = createTestIo({ isTTY: true, columns: 100, rows: 30, env: daemon.env, homeDir: daemon.dir });
    assert.equal(await run(['attach', gone], io), 1);
    assert.match(io.err(), /すでに終了しています/);
    assert.deepEqual(io.rawModes, []);
  } finally {
    await daemon.stop();
  }
});

test('接続が切れたら「接続が切れました」で 1、端末は戻っている', async () => {
  const daemon = await startTestDaemon();
  const a = await openTestPane(daemon.client, ECHO('dc'), { labels: { name: 'dc' } });
  const { io, end } = await startLoop(daemon, a);
  await waitFor(() => io.out().includes('marker-dc'));
  await daemon.stop();
  assert.equal(await end, 1);
  assert.match(io.err(), /接続が切れました/);
  assert.deepEqual(io.rawModes, [true, false]);
});

test('--readonly の一覧には n を出さない', async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ECHO('ro'), { labels: { name: 'ro' } });
    const { io, end } = await startLoop(daemon, a, { readonly: true });
    await waitFor(() => bannerCount(io) === 1);
    io.stdin.write(key('d'));
    await waitFor(() => io.out().includes('番号で入る'));
    assert.doesNotMatch(io.out(), /n で新しいペイン/);
    io.stdin.write('q\n');
    assert.equal(await end, 0);
    await daemon.client.request('pane.close', { paneId: a });
  } finally {
    await daemon.stop();
  }
});

test('attach コマンド: 端末でなければ 2、--no-replay と --readonly を受け付ける', async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ECHO('cmd'), { labels: { name: 'attach-cmd' } });
    const piped = createTestIo({ env: daemon.env, homeDir: daemon.dir });
    assert.equal(await run(['attach', 'attach-cmd'], piped), 2);
    assert.match(piped.err(), /TTY/);

    const io = createTestIo({ isTTY: true, columns: 100, rows: 30, env: daemon.env, homeDir: daemon.dir });
    const done = run(['attach', 'attach-cmd', '--no-replay', '--readonly'], io);
    await waitFor(() => io.err().includes('READONLY'));
    io.stdin.write(key('d'));
    await waitFor(() => io.out().includes('番号で入る'));
    io.stdin.write('q\n');
    assert.equal(await done, 0);
    assert.equal(io.out().includes('marker-cmd'), false, '--no-replay は過去の画面を再生しない');
    assert.equal(await run(['attach'], createTestIo({ isTTY: true, columns: 80, rows: 24, env: daemon.env })), 2);
    await daemon.client.request('pane.close', { paneId: a });
  } finally {
    await daemon.stop();
  }
});

test('new --attach: 作ったペインに入る。--json とは併用できない', async () => {
  const daemon = await startTestDaemon();
  try {
    const io = createTestIo({ isTTY: true, columns: 100, rows: 30, env: daemon.env, homeDir: daemon.dir, cwd: daemon.dir });
    const done = run(['new', '--attach', '--label', 'name=fresh', '--', 'sh', '-c', 'echo fresh-marker; sleep 60'], io);
    await waitFor(() => io.out().includes('fresh-marker'));
    assert.match(io.err(), /\(未登録\) fresh/);
    io.stdin.write(key('d'));
    await waitFor(() => io.out().includes('番号で入る'));
    io.stdin.write('q\n');
    assert.equal(await done, 0);
    assert.doesNotMatch(io.out(), /作成しました/, '--attach では案内を出さずに入る');
    const panes = await daemon.client.request('pane.list', {});
    assert.deepEqual(panes.map((p) => p.labels.origin), ['terminal']);
    await daemon.client.request('pane.close', { paneId: panes[0]!.paneId });

    const json = createTestIo({ isTTY: true, columns: 100, rows: 30, env: daemon.env });
    assert.equal(await run(['new', '--attach', '--json'], json), 2);
    assert.equal(await run(['new', '--attach'], createTestIo({ env: daemon.env })), 2, '端末でなければペインを作らない');
    assert.deepEqual(await daemon.client.request('pane.list', {}), []);
  } finally {
    await daemon.stop();
  }
});
