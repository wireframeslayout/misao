import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { after, before, describe, test } from 'node:test';
import type { EventParams } from '@misao/protocol';
import type { MisaoClient } from '@misao/sdk';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { IS_WINDOWS, nodeEval, skipOnWindows } from './helpers/platform.js';
import { waitFor, waitQuiet } from './helpers/wait.js';

// Node 版と Rust 版 (MISAO_DAEMON_BIN) の両方で通る、第 3 段の範囲（画面・タイトル・稼働判定のイベント・前面プロセス）の契約。
// pane.attach を使わない。attach を使う v2-size-conflict の「画面」の部分（サイズの所有者に画面が追従する）はここで確かめる。

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(() => daemon.stop());

const sh = (script: string): string[] => ['sh', '-c', script];

/** Windows ではシェルの代わりに node で同じことをする。 */
const js = (script: string): string[] => nodeEval(script);

async function screen(paneId: string): ReturnType<typeof client.request<'pane.screen'>> {
  return client.request('pane.screen', { paneId });
}

describe('pane.screen と pane.info', () => {
  test('見えている行・カーソル・タイトル・activity。全角文字は 2 桁、行末の空のセルは含まない', async () => {
    const script = "printf '\\033]0;e2e title\\007one\\r\\n日本\\033[3;1Hz'; sleep 30";
    const winScript = "process.stdout.write('\\x1b]0;e2e title\\x07one\\r\\n日本\\x1b[3;1Hz'); setTimeout(() => {}, 30000)";
    const { paneId } = await client.request('pane.open', { cmd: IS_WINDOWS ? js(winScript) : sh(script), cols: 20, rows: 4 });
    await waitFor(async () => (await screen(paneId)).text.includes('z'), 'the output');
    // ConPTY は子のタイトルを内容より後に届ける (Unix は先)
    await waitFor(async () => (await screen(paneId)).title === 'e2e title', 'the title');
    const first = await screen(paneId);
    assert.equal(first.text, 'one\n日本\nz\n');
    assert.deepEqual(first.cursor, { x: 1, y: 2 });
    assert.equal(first.altScreen, false);
    assert.equal(first.title, 'e2e title');
    assert.equal((await client.request('pane.info', { paneId })).title, 'e2e title');

    await client.request('pane.resize', { paneId, cols: 30, rows: 6 });
    const resized = await screen(paneId);
    // resize で activity が 1 増える。Windows は ConPTY が画面全体を書き直す出力もあとから届いて増えうる。
    if (IS_WINDOWS) assert.ok(resized.activity > first.activity, 'activity grows');
    else assert.equal(resized.activity, first.activity + 1);
    assert.equal(resized.text.split('\n').length, 6);
    await client.request('pane.close', { paneId });
  });

  test('終了した pane でも pane.screen に答える', async () => {
    const { paneId } = await client.request('pane.open', { cmd: IS_WINDOWS ? js("process.stdout.write('bye')") : sh('printf bye'), cols: 10, rows: 2 });
    await waitFor(async () => (await client.request('pane.info', { paneId })).processState === 'exited', 'the pane to exit');
    assert.equal((await screen(paneId)).text, 'bye\n');
    await client.request('pane.close', { paneId });
  });

  test('前面プロセスのコマンド名 (Linux)。終了したら出さない', { skip: process.platform !== 'linux' }, async () => {
    const { paneId } = await client.request('pane.open', { cmd: sh('exec sleep 30') });
    await waitFor(async () => (await client.request('pane.info', { paneId })).fgCommand === 'sleep', 'fgCommand to be sleep');
    await client.request('pane.close', { paneId });
    const { paneId: done } = await client.request('pane.open', { cmd: sh('exit 0') });
    await waitFor(async () => (await client.request('pane.info', { paneId: done })).processState === 'exited', 'the pane to exit');
    assert.equal((await client.request('pane.info', { paneId: done })).fgCommand, undefined);
  });
});

describe('vim の画面がサイズの所有者に追従する（v2-size-conflict の画面の部分）', () => {
  test('最後に入力したクライアントのサイズになり、画面の行数と幅はそのサイズに収まる', { timeout: 60_000, ...skipOnWindows('vim が要る') }, async () => {
    const file = path.join(daemon.dir, 'big.txt');
    fs.writeFileSync(file, Array.from({ length: 200 }, (_, i) => `line ${i} ` + 'x'.repeat(50)).join('\n') + '\n');
    const { paneId } = await client.request('pane.open', { cmd: ['vim', '-u', 'NONE', '-N', file], cols: 100, rows: 30 });
    await waitFor(async () => (await screen(paneId)).altScreen, 'vim to enter the alt screen');
    const A = { cols: 120, rows: 40 };
    const B = { cols: 80, rows: 24 };
    await client.request('pane.resize', { paneId, ...A, clientId: 'A' });
    await client.request('pane.resize', { paneId, ...B, clientId: 'B' });
    assert.equal((await client.request('pane.info', { paneId })).sizeOwner, 'B');

    for (const [owner, size] of [['A', A], ['B', B], ['A', A]] as const) {
      // 入力した側が所有者になり、サイズが戻る。再描画を起こすキー (l) を送る。
      await client.request('pane.write', { paneId, data: 'l', clientId: owner, source: 'terminal' });
      await waitFor(async () => {
        const info = await client.request('pane.info', { paneId });
        return info.cols === size.cols && info.rows === size.rows && info.sizeOwner === owner;
      }, `${owner}'s size`);
      await waitQuiet(client, paneId, { quietMs: 500, timeoutMs: 15_000 });
      const current = await screen(paneId);
      const rows = current.text.split('\n');
      assert.equal(rows.length, size.rows, `${owner}: rows`);
      assert.ok(rows.every((row) => [...row].length <= size.cols), `${owner}: no row wider than ${size.cols}`);
      assert.ok(current.altScreen && current.cursor.y < size.rows && current.cursor.x <= size.cols, `${owner}: cursor ${JSON.stringify(current.cursor)}`);
      assert.match(rows.join('\n'), /line \d+ x+/);
    }
    await client.request('pane.close', { paneId });
  });
});

describe('稼働判定のイベント', () => {
  test('スピナーのタイトルで working（title 段）、通常のタイトルで idle、終了で exited。state は title より先に出る', async () => {
    const { epoch } = await client.request('server.info', {});
    const events: EventParams[] = [];
    const sub = await daemon.connect();
    await sub.subscribeEvents((e) => events.push(e), { since: (await client.request('server.info', {})).eventHead, epoch });
    const script = "printf '\\033]0;⠋ build\\007'; sleep 0.3; printf '\\033]0;done\\007'; sleep 0.3";
    const winScript = "process.stdout.write('\\x1b]0;⠋ build\\x07'); setTimeout(() => { process.stdout.write('\\x1b]0;done\\x07'); setTimeout(() => {}, 300); }, 300)";
    const { paneId } = await client.request('pane.open', { cmd: IS_WINDOWS ? js(winScript) : sh(script) });
    await waitFor(() => events.some((e) => e.type === 'pane.state' && e.paneId === paneId && (e.data as { state: string }).state === 'exited'), 'exited');
    // ConPTY は子が始まる前に、実行ファイルのパスを自分のタイトルとして出す。
    const isConPtyTitle = (e: EventParams): boolean =>
      IS_WINDOWS && e.type === 'pane.title' && String((e.data as { title?: string }).title).toLowerCase() === process.execPath.toLowerCase();
    const mine = events.filter((e) => e.paneId === paneId && (e.type === 'pane.title' || e.type === 'pane.state' || e.type === 'pane.exited') && !isConPtyTitle(e));
    const summary = mine.map((e) => {
      const data = e.data as { state?: string; decidedBy?: string; prev?: string; title?: string };
      return e.type === 'pane.title' ? `title:${data.title}` : e.type === 'pane.exited' ? 'exited' : `${data.prev}>${data.state}:${data.decidedBy}`;
    });
    assert.deepEqual(summary, [
      'unknown>working:title',
      'title:⠋ build',
      'working>idle:title',
      'title:done',
      'exited',
      'idle>exited:exit',
    ]);
    await client.request('pane.close', { paneId });
    sub.close();
  });
});
