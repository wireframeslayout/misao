import assert from 'node:assert/strict';
import { after, before, describe, test } from 'node:test';
import type { PaneInfo } from '@misao/protocol';
import { run } from '../src/run.js';
import { createTestIo } from './helpers/io.js';
import { openTestPane, startTestDaemon, waitFor } from './helpers/daemon.js';
import type { TestDaemon } from './helpers/daemon.js';

interface Result {
  code: number;
  out: string;
  err: string;
}

async function misao(daemon: TestDaemon | undefined, argv: string[], stdin?: string): Promise<Result> {
  const io = createTestIo({
    env: daemon?.env ?? { MISAO_SOCKET: '/tmp/misao-test-no-such-daemon/misao.sock' },
    homeDir: daemon?.dir ?? '/tmp/misao-test-no-home',
  });
  if (stdin !== undefined) io.stdin.end(stdin);
  const code = await run(argv, io);
  return { code, out: io.out(), err: io.err() };
}

const SLEEP = ['sh', '-c', 'sleep 60'];

describe('引数まわり (デーモン不要)', () => {
  test('--version は misao 0.0.0 を出して 0', async () => {
    const r = await misao(undefined, ['--version']);
    assert.deepEqual([r.code, r.out], [0, 'misao 0.0.0\n']);
  });

  test('コマンドなしは使い方の誤り (2)、--help は 0', async () => {
    const none = await misao(undefined, []);
    assert.equal(none.code, 2);
    assert.match(none.err, /コマンドを指定してください/);
    const help = await misao(undefined, ['--help']);
    assert.equal(help.code, 0);
    for (const name of ['ls', 'send', 'screen', 'label', 'status', 'schema']) {
      assert.match(help.out, new RegExp(`^  ${name} `, 'm'), name);
    }
  });

  test('不明なコマンド・不明なオプションは 2、--json ならエラーも JSON', async () => {
    const unknown = await misao(undefined, ['frobnicate']);
    assert.equal(unknown.code, 2);
    const option = await misao(undefined, ['ls', '--nope', '--json']);
    assert.equal(option.code, 2);
    assert.equal((JSON.parse(option.err) as { error: { code: string } }).error.code, 'usage');
  });

  test('デーモンが無いときは 1。案内つき、--json なら daemon_unreachable', async () => {
    const human = await misao(undefined, ['ls']);
    assert.equal(human.code, 1);
    assert.match(human.err, /misao serve/);
    const json = await misao(undefined, ['ls', '--json']);
    assert.equal(json.code, 1);
    assert.equal((JSON.parse(json.err) as { error: { code: string } }).error.code, 'daemon_unreachable');
  });

  test('status は停止中を表示して 1 (--json は running:false)', async () => {
    const human = await misao(undefined, ['status']);
    assert.equal(human.code, 1);
    assert.match(human.out, /停止中/);
    const json = await misao(undefined, ['status', '--json']);
    assert.equal(json.code, 1);
    assert.equal((JSON.parse(json.out) as { running: boolean }).running, false);
  });
});

describe('実デーモンに対するコマンド', () => {
  let daemon: TestDaemon;
  before(async () => {
    daemon = await startTestDaemon();
  });
  after(async () => {
    await daemon.stop();
  });

  test('status: 稼働中の情報 (人向けと --json)', async () => {
    const human = await misao(daemon, ['status']);
    assert.equal(human.code, 0);
    assert.match(human.out, /稼働中/);
    assert.match(human.out, new RegExp(daemon.socketPath.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
    const json = JSON.parse((await misao(daemon, ['status', '--json'])).out) as { running: boolean; pid: number; epoch: string; socket: string };
    assert.equal(json.running, true);
    assert.equal(json.pid, process.pid);
    assert.equal(json.socket, daemon.socketPath);
  });

  test('schema: server.schema の JSON をそのまま出す', async () => {
    const r = await misao(daemon, ['schema']);
    assert.equal(r.code, 0);
    const direct = await daemon.client.request('server.schema', {});
    assert.deepEqual(JSON.parse(r.out), direct);
  });

  test('ls / label / screen / send を 1 つのペインに対して通しで使う', async () => {
    const paneId = await openTestPane(
      daemon.client,
      ['sh', '-c', 'echo ready-marker; read x; echo got:$x; sleep 60'],
      { labels: { name: '通しテスト', task: '437', windowId: '901' } },
    );

    const ls = await misao(daemon, ['ls']);
    assert.equal(ls.code, 0);
    assert.match(ls.out, /^STATE +PANE +NAME +TASK +AGENT +CWD +LAST$/m);
    assert.match(ls.out, /W-901 · 通しテスト/);
    assert.match(ls.out, /#437/);

    const filtered = JSON.parse((await misao(daemon, ['ls', '--json', '--task', '437'])).out) as PaneInfo[];
    assert.deepEqual(filtered.map((p) => p.paneId), [paneId]);
    const none = JSON.parse((await misao(daemon, ['ls', '--json', '--task', '9999'])).out) as PaneInfo[];
    assert.deepEqual(none, []);
    const byWorkspace = JSON.parse((await misao(daemon, ['ls', '--json', '--workspace', 'no-such'])).out) as PaneInfo[];
    assert.deepEqual(byWorkspace, []);
    assert.equal((await misao(daemon, ['ls', '--state', 'bogus'])).code, 2);

    const label = await misao(daemon, ['label', 'W-901', 'owner=me', '--unset', 'task']);
    assert.equal(label.code, 0);
    assert.match(label.out, /^owner=me$/m);
    assert.doesNotMatch(label.out, /task=/);
    const labelJson = JSON.parse((await misao(daemon, ['label', '901', 'k=v', '--json'])).out) as { labels: Record<string, string> };
    assert.equal(labelJson.labels.k, 'v');
    assert.equal((await misao(daemon, ['label', '901'])).code, 2, '変更が無ければ使い方の誤り');
    assert.equal((await misao(daemon, ['label', '901', 'novalue'])).code, 2);

    await waitFor(async () => (await misao(daemon, ['screen', '901'])).out.includes('ready-marker'));
    const send = await misao(daemon, ['send', '901', 'hello', '--enter']);
    assert.equal(send.code, 0);
    await waitFor(async () => (await misao(daemon, ['screen', '901'])).out.includes('got:hello'));

    const last = await misao(daemon, ['screen', '901', '--lines', '1']);
    assert.equal(last.out.trim().split('\n').length, 1);
    const screenJson = JSON.parse((await misao(daemon, ['screen', '901', '--json'])).out) as { text: string; altScreen: boolean };
    assert.match(screenJson.text, /got:hello/);
    assert.equal((await misao(daemon, ['screen', '901', '--lines', '0'])).code, 2);

    await daemon.client.request('pane.close', { paneId });
  });

  test('send: --keys と --stdin', async () => {
    const paneId = await openTestPane(daemon.client, ['sh', '-c', 'read a; echo A:$a; read b; echo B:$b; sleep 60'], {
      labels: { name: 'send-keys' },
    });
    const viaKeys = await misao(daemon, ['send', 'send-keys', 'one', '--keys', 'Enter']);
    assert.equal(viaKeys.code, 0);
    await waitFor(async () => (await misao(daemon, ['screen', 'send-keys'])).out.includes('A:one'));
    const viaStdin = await misao(daemon, ['send', 'send-keys', '--stdin'], 'two\n');
    assert.equal(viaStdin.code, 0);
    await waitFor(async () => (await misao(daemon, ['screen', 'send-keys'])).out.includes('B:two'));
    assert.equal((await misao(daemon, ['send', 'send-keys', '--keys', 'NoSuchKey'])).code, 2);
    assert.equal((await misao(daemon, ['send', 'send-keys'])).code, 2, '送る内容が無い');
    await daemon.client.request('pane.close', { paneId });
  });

  test('対象が曖昧なら 1 で候補を出し、無ければ not_found', async () => {
    const a = await openTestPane(daemon.client, SLEEP, { labels: { name: 'twin one' } });
    const b = await openTestPane(daemon.client, SLEEP, { labels: { name: 'twin two' } });
    const ambiguous = await misao(daemon, ['screen', 'twin']);
    assert.equal(ambiguous.code, 1);
    assert.ok(ambiguous.err.includes(a) && ambiguous.err.includes(b), ambiguous.err);
    const json = JSON.parse((await misao(daemon, ['screen', 'twin', '--json'])).err) as {
      error: { code: string; candidates: Array<{ paneId: string }> };
    };
    assert.equal(json.error.code, 'ambiguous');
    assert.deepEqual(json.error.candidates.map((c) => c.paneId).sort(), [a, b].sort());
    const missing = await misao(daemon, ['screen', 'nothing-matches']);
    assert.equal(missing.code, 1);
    assert.match(missing.err, /当てはまるペインがありません/);
    await daemon.client.request('pane.close', { paneId: a });
    await daemon.client.request('pane.close', { paneId: b });
  });
});
