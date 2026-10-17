import assert from 'node:assert/strict';
import { test } from 'node:test';
import { TTY_RESET } from '../../src/attach/tty.js';
import { openTestPane, startTestDaemon, waitFor } from '../helpers/daemon.js';
import type { TestDaemon } from '../helpers/daemon.js';
import { CLI_BIN, startMisao } from '../helpers/runner.js';
import type { Running } from '../helpers/runner.js';

// attach の端末まわりを、疑似端末の中で動かした `MISAO_CLI_BIN` のバイナリに対して確かめる。
// (session.test.ts / loop.test.ts は Node 版の内部関数をテストするので、バイナリは対象にならない。)
const skip = CLI_BIN === undefined ? 'MISAO_CLI_BIN が未設定' : false;

const PREFIX_D = '\x1ed';
const key = (k: string): string => `\x1e${k}`;
const ECHO = (name: string): string[] => ['sh', '-c', `echo marker-${name}; sleep 60`];
const READ_ONE = ['sh', '-c', 'echo ready-marker; read x; echo got:$x; sleep 60'];
const PICKER = '番号で入る';

function attach(daemon: TestDaemon, argv: string[], size = { cols: 100, rows: 30 }): Running {
  return startMisao(argv, { env: daemon.env, homeDir: daemon.dir, cwd: daemon.dir, isTTY: true, ...size });
}

const bannerCount = (r: Running): number => r.out().split('\n').filter((l) => l.includes('[misao] ') && l.includes('抜ける:')).length;
const pickerCount = (r: Running): number => r.out().split(PICKER).length - 1;

test('attach: 入出力をつなぎ、サイズに追従し、prefix + d で一覧に戻って q で終了 0。端末を戻す', { skip }, async () => {
  const daemon = await startTestDaemon();
  try {
    const paneId = await openTestPane(daemon.client, READ_ONE, { labels: { windowId: '806', name: 'セッション', task: '419' } });
    const r = attach(daemon, ['attach', 'W-806']);
    await waitFor(() => r.out().includes('ready-marker'));
    assert.match(r.out(), /\[misao\] W-806 · セッション · task #419 · sh · \w+ · 抜ける: Ctrl-\^ d/);
    let info = await daemon.client.request('pane.info', { paneId });
    assert.deepEqual([info.cols, info.rows], [100, 30]);
    assert.ok(info.clients.some((c) => c.startsWith('cli-')));
    r.write('abc\r');
    await waitFor(() => r.out().includes('got:abc'));
    r.resize(120, 40);
    await waitFor(async () => (await daemon.client.request('pane.info', { paneId })).cols === 120);
    r.write(PREFIX_D);
    await waitFor(() => r.out().includes(PICKER));
    assert.match(r.out(), /W-806 から抜けました（入力 1 回を記録。内容は保存していません）/);
    assert.ok(r.out().includes(TTY_RESET), 'TTY_RESET を書く');
    await waitFor(async () => (await daemon.client.request('pane.info', { paneId })).clients.length === 0);
    r.write('q\r');
    assert.equal(await r.done, 0);
    info = await daemon.client.request('pane.info', { paneId });
    assert.deepEqual(info.clients, []);
    await daemon.client.request('pane.close', { paneId });
  } finally {
    await daemon.stop();
  }
});

test('attach: n / p は一覧の隣へ循環して移動し、l は「抜けました」なしで一覧へ、番号で別のペインへ入れる', { skip }, async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ECHO('a'), { labels: { name: 'loop-a' } });
    await waitFor(async () => (await daemon.client.request('pane.screen', { paneId: a })).text.includes('marker-a'));
    const b = await openTestPane(daemon.client, ECHO('b'), { labels: { name: 'loop-b' } });
    await waitFor(async () => (await daemon.client.request('pane.screen', { paneId: b })).text.includes('marker-b'));
    const r = attach(daemon, ['attach', 'loop-a']);
    await waitFor(() => bannerCount(r) === 1);
    r.write(key('n'));
    await waitFor(() => bannerCount(r) === 2);
    r.write(key('p'));
    await waitFor(() => bannerCount(r) === 3);
    r.write(key('l'));
    await waitFor(() => r.out().includes(PICKER));
    assert.doesNotMatch(r.out(), /から抜けました/);
    const row = r.out().split('\n').find((l) => /^\s*\d+\s/.test(l) && l.includes('loop-b'))!;
    r.write(`${/^\s*(\d+)/.exec(row)![1]}\r`);
    await waitFor(() => bannerCount(r) === 4);
    r.write(key('d'));
    await waitFor(() => pickerCount(r) === 2);
    r.write('q\r');
    assert.equal(await r.done, 0);
    await daemon.client.request('pane.close', { paneId: a });
    await daemon.client.request('pane.close', { paneId: b });
  } finally {
    await daemon.stop();
  }
});

test('attach: 一覧の n は origin=terminal の新しいシェルを作って入る。--readonly の一覧には n を出さない', { skip }, async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ECHO('base'), { labels: { name: 'base' } });
    const r = attach(daemon, ['attach', 'base']);
    await waitFor(() => bannerCount(r) === 1);
    r.write(key('l'));
    await waitFor(() => r.out().includes(PICKER));
    r.write('n\r');
    await waitFor(() => bannerCount(r) === 2);
    const panes = await daemon.client.request('pane.list', {});
    const created = panes.find((p) => p.paneId !== a)!;
    assert.equal(created.labels.origin, 'terminal');
    assert.equal(created.cmd[1], '-l');
    r.write(key('d'));
    await waitFor(() => pickerCount(r) === 2);
    r.write('q\r');
    assert.equal(await r.done, 0);

    const ro = attach(daemon, ['attach', 'base', '--readonly']);
    await waitFor(() => bannerCount(ro) === 1);
    assert.match(ro.out(), /READONLY/);
    ro.write(key('d'));
    await waitFor(() => ro.out().includes(PICKER));
    assert.doesNotMatch(ro.out(), /n で新しいペイン/);
    ro.write('q\r');
    assert.equal(await ro.done, 0);
    for (const p of panes) await daemon.client.request('pane.close', { paneId: p.paneId });
  } finally {
    await daemon.stop();
  }
});

test('attach --readonly は入力を送らず resize もしない', { skip }, async () => {
  const daemon = await startTestDaemon();
  try {
    const paneId = await openTestPane(daemon.client, READ_ONE, { labels: { name: 'ro' } });
    const r = attach(daemon, ['attach', 'ro', '--readonly']);
    await waitFor(() => r.out().includes('ready-marker'));
    r.write('typed\r');
    r.resize(150, 50);
    r.write(PREFIX_D);
    await waitFor(() => r.out().includes('入力 0 回を記録'));
    const info = await daemon.client.request('pane.info', { paneId });
    assert.deepEqual([info.cols, info.rows], [80, 24]);
    assert.doesNotMatch((await daemon.client.request('pane.screen', { paneId })).text, /got:/);
    r.write('q\r');
    assert.equal(await r.done, 0);
    await daemon.client.request('pane.close', { paneId });
  } finally {
    await daemon.stop();
  }
});

test('attach: ペインが終了したら一覧に戻り、入れるペインが無ければ 0。終了済みのペインには入らず 1', { skip }, async () => {
  const daemon = await startTestDaemon();
  try {
    await openTestPane(daemon.client, ['sh', '-c', 'sleep 0.5; exit 2'], { labels: { windowId: '77' } });
    const r = attach(daemon, ['attach', 'W-77']);
    assert.equal(await r.done, 0);
    assert.match(r.out(), /W-77 が終了しました（終了コード 2）/);
    assert.ok(r.out().includes(TTY_RESET));

    const gone = await openTestPane(daemon.client, ['sh', '-c', 'exit 0']);
    await waitFor(async () => (await daemon.client.request('pane.info', { paneId: gone })).processState === 'exited');
    const g = attach(daemon, ['attach', gone]);
    assert.equal(await g.done, 1);
    assert.match(g.out(), /すでに終了しています/);
  } finally {
    await daemon.stop();
  }
});

test('attach: 終了シグナルで 1、デーモンが落ちたら「接続が切れました」で 1。どちらも端末を戻す', { skip }, async () => {
  const daemon = await startTestDaemon();
  const a = await openTestPane(daemon.client, ECHO('sig'), { labels: { name: 'sig' } });
  const r = attach(daemon, ['attach', 'sig']);
  await waitFor(() => r.out().includes('marker-sig'));
  r.signal('SIGTERM');
  assert.equal(await r.done, 1);
  assert.match(r.out(), /終了シグナルを受けたため抜けました/);
  assert.ok(r.out().endsWith(TTY_RESET) || r.out().includes(TTY_RESET));

  const d = attach(daemon, ['attach', 'sig']);
  await waitFor(() => d.out().includes('marker-sig'));
  await daemon.stop();
  assert.equal(await d.done, 1);
  assert.match(d.out(), /接続が切れました/);
  void a;
});

test('new --attach: 作ったペインに入る。端末でなければペインを作らない', { skip }, async () => {
  const daemon = await startTestDaemon();
  try {
    const r = attach(daemon, ['new', '--attach', '--label', 'name=fresh', '--', 'sh', '-c', 'echo fresh-marker; sleep 60']);
    await waitFor(() => r.out().includes('fresh-marker'));
    assert.match(r.out(), /\(未登録\) fresh/);
    r.write(key('d'));
    await waitFor(() => r.out().includes(PICKER));
    r.write('q\r');
    assert.equal(await r.done, 0);
    assert.doesNotMatch(r.out(), /作成しました/);
    const panes = await daemon.client.request('pane.list', {});
    assert.deepEqual(panes.map((p) => p.labels.origin), ['terminal']);
    await daemon.client.request('pane.close', { paneId: panes[0]!.paneId });

    const piped = startMisao(['new', '--attach'], { env: daemon.env, homeDir: daemon.dir });
    assert.equal(await piped.done, 2);
    assert.deepEqual(await daemon.client.request('pane.list', {}), []);
  } finally {
    await daemon.stop();
  }
});
