import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { after, before, describe, test } from 'node:test';
import type { EventParams } from '@misao/protocol';
import { ErrorCode } from '@misao/protocol';
import type { MisaoClient } from '@misao/sdk';
import { MisaoRpcError } from '@misao/sdk';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { IS_WINDOWS, nodeEval, sleepCmd } from './helpers/platform.js';
import { sleep, waitFor } from './helpers/wait.js';

// Node 版と Rust 版 (MISAO_DAEMON_BIN) の両方で通る、第 2 段の範囲（作業場所 / 窓 / ペイン、PTY、行の記録、イベント）の契約。
// 画面・attach・稼働判定は含めない。

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(() => daemon.stop());

async function rpcCode(promise: Promise<unknown>): Promise<number> {
  try {
    await promise;
  } catch (error) {
    assert.ok(error instanceof MisaoRpcError, `expected an RPC error: ${String(error)}`);
    return error.code;
  }
  throw new Error('expected the request to fail');
}

const sh = (script: string): string[] => ['sh', '-c', script];

/** Windows ではシェルの代わりに node で同じことをする（ConPTY の中で動く）。JSON.stringify でパスを安全に埋め込む。 */
const js = (script: string): string[] => nodeEval(script);

/** 行の入力を確定するキー。Windows のコンソールは LF では確定せず、Enter (CR) が要る。 */
const ENTER = IS_WINDOWS ? '\r' : '\n';

async function waitForExit(paneId: string): Promise<Awaited<ReturnType<typeof client.request<'pane.info'>>>> {
  await waitFor(async () => (await client.request('pane.info', { paneId })).processState === 'exited', `pane ${paneId} to exit`);
  return client.request('pane.info', { paneId });
}

/** file が lines 行（改行終端）になるまで待って、その中身を返す。 */
async function readLines(file: string, lines: number): Promise<string> {
  const count = (): number => (fs.existsSync(file) ? fs.readFileSync(file, 'utf8').split('\n').length - 1 : 0);
  await waitFor(() => count() >= lines, `${file} to have ${lines} lines`);
  return fs.readFileSync(file, 'utf8').trimEnd();
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

describe('作業場所と窓', () => {
  test('作成・一覧・改名・削除と、現行と同じエラーコード', async () => {
    const created = await client.request('workspace.create', { name: 'e2e-ws' });
    assert.deepEqual(created, { name: 'e2e-ws', windows: [] });
    const window = await client.request('window.create', { workspace: 'e2e-ws', name: 'main' });
    assert.match(window.windowId, /^w_[0-7][0-9A-HJKMNP-TV-Z]{25}$/);
    await client.request('window.rename', { windowId: window.windowId, name: 'renamed' });
    await client.request('workspace.rename', { name: 'e2e-ws', newName: 'e2e-ws2' });
    const listed = (await client.request('workspace.list', {})).find((w) => w.name === 'e2e-ws2');
    assert.deepEqual(listed?.windows, [{ windowId: window.windowId, name: 'renamed', workspace: 'e2e-ws2' }]);

    assert.equal(await rpcCode(client.request('workspace.create', { name: 'e2e-ws2' })), ErrorCode.AlreadyExists);
    assert.equal(await rpcCode(client.request('workspace.close', { name: 'no-such' })), ErrorCode.WorkspaceNotFound);
    assert.equal(await rpcCode(client.request('window.close', { windowId: `w_${'0'.repeat(26)}` })), ErrorCode.WindowNotFound);

    await client.request('window.close', { windowId: window.windowId });
    await client.request('workspace.close', { name: 'e2e-ws2' });
    assert.ok(!(await client.request('workspace.list', {})).some((w) => w.name === 'e2e-ws2'));
  });
});

describe('ペイン', () => {
  test('起動・env の継承と ephemeralEnv・cwd・終了コード', async () => {
    const out = path.join(daemon.dir, 'env.out');
    const winScript = `const e = process.env; require('fs').writeFileSync(${JSON.stringify(out)}, process.cwd() + '\\n' + [e.KEPT, e.SECRET, e.MISAO_PANE_ID, e.TERM, e.MISAO_SOCKET].join('|') + '\\n'); process.exit(4)`;
    const { paneId } = await client.request('pane.open', {
      cmd: IS_WINDOWS ? js(winScript) : sh(`pwd > ${out}; echo "$KEPT|$SECRET|$MISAO_PANE_ID|$TERM|$MISAO_SOCKET" >> ${out}; exit 4`),
      cwd: daemon.dir,
      env: { KEPT: 'env-value', SECRET: 'persisted' },
      ephemeralEnv: { SECRET: 'ephemeral' },
      labels: { owner: 'e2e' },
    });
    const [cwd, vars] = (await readLines(out, 2)).split('\n');
    assert.equal(fs.realpathSync(cwd!), fs.realpathSync(daemon.dir));
    assert.equal(vars, `env-value|ephemeral|${paneId}|xterm-256color|${daemon.socket}`);
    const info = await waitForExit(paneId);
    assert.deepEqual([info.exitCode, info.signal], [4, null]);
    assert.deepEqual(info.labels, { owner: 'e2e' });
    assert.ok(!JSON.stringify(info).includes('ephemeral'), 'ephemeralEnv は報告しない');
    assert.ok((await client.request('pane.list', { filter: { state: 'exited', labels: { owner: 'e2e' } } })).some((p) => p.paneId === paneId));
    await client.request('pane.close', { paneId });
    assert.equal(await rpcCode(client.request('pane.info', { paneId })), ErrorCode.PaneNotFound);
  });

  test('入力と resize', async () => {
    const out = path.join(daemon.dir, 'tty.out');
    // stty size は「行 列」。Windows は node が端末の大きさ（getWindowSize は [列, 行]）を同じ形で書く。
    const winScript = `const fs = require('fs'); const size = () => { const [c, r] = process.stdout.getWindowSize(); return r + ' ' + c; };
      fs.appendFileSync(${JSON.stringify(out)}, size() + '\\n'); process.stdin.once('data', (d) => { fs.appendFileSync(${JSON.stringify(out)}, 'got:' + String(d).trim() + '\\n' + size() + '\\n'); process.exit(0); })`;
    const { paneId } = await client.request('pane.open', {
      cmd: IS_WINDOWS ? js(winScript) : sh(`stty size > ${out}; read a; echo "got:$a" >> ${out}; stty size >> ${out}`),
      cols: 100,
      rows: 30,
    });
    assert.equal(await readLines(out, 1), '30 100');
    await client.request('pane.resize', { paneId, cols: 70, rows: 20, clientId: 'A' });
    const resized = await client.request('pane.info', { paneId });
    assert.deepEqual([resized.cols, resized.rows, resized.sizeOwner], [70, 20, 'A']);
    await client.request('pane.write', { paneId, dataB64: Buffer.from(`hello${ENTER}`).toString('base64') });
    assert.equal(await readLines(out, 3), '30 100\ngot:hello\n20 70');
    await waitForExit(paneId);
    assert.equal(await rpcCode(client.request('pane.write', { paneId, data: 'x' })), ErrorCode.PaneExited);
    await client.request('pane.close', { paneId });
  });

  test('close はバックグラウンドの子も含めて止める', async () => {
    const pidFile = path.join(daemon.dir, 'bg.pid');
    // Windows: 孫プロセス（node が起動した、さらに別の node）が Job Object で止まること
    const winScript = `const c = require('child_process').spawn(process.execPath, ['-e', 'setTimeout(() => {}, 300000)'], { stdio: 'ignore' }); require('fs').writeFileSync(${JSON.stringify(pidFile)}, c.pid + '\\n'); setTimeout(() => {}, 300000)`;
    const { paneId } = await client.request('pane.open', { cmd: IS_WINDOWS ? js(winScript) : sh(`sleep 300 & echo $! > ${pidFile}; wait`) });
    const child = Number(await readLines(pidFile, 1));
    assert.ok(isAlive(child));
    await client.request('pane.close', { paneId });
    await waitFor(() => !isAlive(child), 'the background child to be killed', { timeoutMs: 15_000 });
  });

  test('窓・作業場所を閉じると配下のペインも閉じる', async () => {
    await client.request('workspace.create', { name: 'doomed' });
    const window = await client.request('window.create', { workspace: 'doomed', name: 'w' });
    const { paneId } = await client.request('pane.open', { cmd: sleepCmd(), windowId: window.windowId });
    await client.request('workspace.close', { name: 'doomed' });
    assert.equal(await rpcCode(client.request('pane.info', { paneId })), ErrorCode.PaneNotFound);
  });
});

describe('行の記録', () => {
  test('ANSI を除いた行が since=0 から届き、最後の改行なしの行も流れる', async () => {
    const { epoch } = await client.request('server.info', {});
    const printed = String.raw`printf '\033[31mred\033[0m a\r\n\033]0;t\007b\nlast'`;
    const winPrint = `process.stdout.write('\\x1b[31mred\\x1b[0m a\\r\\n\\x1b]0;t\\x07b\\nlast')`;
    const { paneId } = await client.request('pane.open', { cmd: IS_WINDOWS ? js(winPrint) : sh(printed) });
    const lines: Array<{ seq: number; text: string }> = [];
    const sub = await daemon.connect();
    await sub.subscribeLines(paneId, (l) => lines.push({ seq: l.seq, text: l.text }), { since: 0, epoch });
    await waitFor(() => lines.length >= 3, 'three lines');
    assert.deepEqual(lines, [
      { seq: 1, text: 'red a' },
      { seq: 2, text: 'b' },
      { seq: 3, text: 'last' },
    ]);
    sub.close();
    await client.request('pane.close', { paneId });
  });

  test('since / epoch / gap: 保持範囲内は gap なし、head より先と別 epoch は gap', async () => {
    const small = await startDaemonProcess({ rings: { linesBytes: 200 } });
    try {
      const c = await small.connect();
      const { epoch } = await c.request('server.info', {});
      const winLines = `for (let i = 1; i <= 5; i++) process.stdout.write('line ' + i + '\\r\\n')`;
      const { paneId } = await c.request('pane.open', { cmd: IS_WINDOWS ? js(winLines) : sh('for i in 1 2 3 4 5; do echo line $i; done') });
      await waitFor(async () => (await c.request('pane.info', { paneId })).processState === 'exited', 'exit');
      const sub = (since: number, e: string = epoch) => c.request('pane.subscribe_lines', { paneId, since, epoch: e });
      assert.deepEqual({ ...(await sub(3)), epoch: undefined }, { gap: false, head: 5, epoch: undefined });
      assert.equal((await sub(1)).gap, true, '追い出された範囲を含む');
      assert.equal((await sub(5)).gap, false);
      assert.equal((await sub(1000)).gap, true, 'head より先');
      assert.equal((await sub(5, '0'.repeat(26))).gap, true, '別の epoch');
    } finally {
      await small.stop();
    }
  });
});

describe('イベント', () => {
  test('daemon.started が 1 件目で、pane の生存期間のイベントが順に届く。入力の内容は含まれない', async () => {
    const { epoch } = await client.request('server.info', {});
    const events: EventParams[] = [];
    const sub = await daemon.connect();
    await sub.subscribeEvents((e) => events.push(e), { since: 0, epoch });
    await waitFor(() => events.length > 0, 'the first event');
    assert.equal(events[0]!.type, 'daemon.started');
    assert.equal(events[0]!.seq, 1);

    const mark = events.length;
    const winRead = `process.stdin.once('data', () => process.exit(0))`;
    const { paneId } = await client.request('pane.open', { cmd: IS_WINDOWS ? js(winRead) : sh('read x; exit 0'), labels: { k: 'v' } });
    await client.request('pane.write', { paneId, data: `secret-input${ENTER}`, source: 'terminal' });
    await waitForExit(paneId);
    await client.request('pane.close', { paneId });
    await waitFor(() => events.some((e) => e.type === 'pane.closed' && e.paneId === paneId), 'pane.closed');
    const mine = events.slice(mark).filter((e) => e.paneId === paneId);
    const types = mine.map((e) => e.type);
    assert.deepEqual(types.filter((t) => t !== 'pane.state'), ['pane.opened', 'input', 'pane.exited', 'pane.closed']);
    assert.deepEqual(mine.find((e) => e.type === 'input')!.data, { source: 'terminal', bytes: 13 });
    assert.ok(!JSON.stringify(events).includes('secret-input'));
    assert.ok(events.every((e, i) => i === 0 || e.seq === events[i - 1]!.seq + 1), 'seq は連続する');
    sub.close();
  });

  test('接続し直しても since で続きから受け取れる', async () => {
    const { epoch } = await client.request('server.info', {});
    const first: EventParams[] = [];
    const a = await daemon.connect();
    await a.subscribeEvents((e) => first.push(e), { since: (await client.request('server.info', {})).eventHead, epoch });
    await client.request('workspace.create', { name: 'resume-1' });
    await waitFor(() => first.length >= 1, 'the first live event');
    const last = first.at(-1)!.seq;
    a.close();
    await client.request('workspace.create', { name: 'resume-2' });
    await client.request('workspace.create', { name: 'resume-3' });

    const resumed: EventParams[] = [];
    const b = await daemon.connect();
    await b.subscribeEvents((e) => resumed.push(e), { since: last, epoch });
    await waitFor(() => resumed.length >= 2, 'the replay');
    await sleep(200);
    assert.deepEqual(resumed.map((e) => e.seq), [last + 1, last + 2]);
    assert.deepEqual(resumed.map((e) => (e.data as { name: string }).name), ['resume-2', 'resume-3']);

    b.close();
    for (const name of ['resume-1', 'resume-2', 'resume-3']) await client.request('workspace.close', { name });
  });
});
