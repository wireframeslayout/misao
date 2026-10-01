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

interface MisaoOptions {
  stdin?: string;
  isTTY?: boolean;
  shell?: string | null;
}

async function misao(daemon: TestDaemon | undefined, argv: string[], stdinOrOptions?: string | MisaoOptions): Promise<Result> {
  const options: MisaoOptions = typeof stdinOrOptions === 'string' ? { stdin: stdinOrOptions } : (stdinOrOptions ?? {});
  const io = createTestIo({
    env: daemon?.env ?? { MISAO_SOCKET: '/tmp/misao-test-no-such-daemon/misao.sock' },
    homeDir: daemon?.dir ?? '/tmp/misao-test-no-home',
    cwd: daemon?.dir ?? '/tmp',
    ...(options.isTTY === undefined ? {} : { isTTY: options.isTTY }),
    ...(options.shell === undefined ? {} : { shell: options.shell }),
  });
  if (options.stdin !== undefined) io.stdin.end(options.stdin);
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
    for (const name of ['ls', 'attach', 'new', 'kill', 'send', 'screen', 'tail', 'events', 'label', 'status', 'schema', 'serve']) {
      assert.match(help.out, new RegExp(`^  ${name} `, 'm'), name);
    }
    assert.match(help.out, /^対象 <target> の書き方:$/m);
    assert.match(help.out, /数字だけの ID は p_ を付けて書く/);
    const sendHelp = await misao(undefined, ['send', '--help']);
    assert.equal(sendHelp.code, 0);
    assert.match(sendHelp.out, /^対象 <target> の書き方:$/m);
    assert.doesNotMatch((await misao(undefined, ['status', '--help'])).out, /対象 <target>/);
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
    const inDefault = JSON.parse((await misao(daemon, ['ls', '--json', '--workspace', 'default'])).out) as PaneInfo[];
    assert.deepEqual(inDefault.map((p) => p.paneId), [paneId]);
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
    const inputs: Array<{ paneId?: string; source?: unknown }> = [];
    const sub = await daemon.client.subscribeEvents((ev) => {
      if (ev.type === 'input') inputs.push({ paneId: ev.paneId, source: ev.data.source });
    });
    const viaKeys = await misao(daemon, ['send', 'send-keys', 'one', '--keys', 'Enter']);
    assert.equal(viaKeys.code, 0);
    await waitFor(() => inputs.some((i) => i.paneId === paneId));
    sub.unsubscribe();
    assert.deepEqual(inputs.filter((i) => i.paneId === paneId).map((i) => i.source), ['terminal']);
    await waitFor(async () => (await misao(daemon, ['screen', 'send-keys'])).out.includes('A:one'));
    const viaStdin = await misao(daemon, ['send', 'send-keys', '--stdin'], 'two\n');
    assert.equal(viaStdin.code, 0);
    await waitFor(async () => (await misao(daemon, ['screen', 'send-keys'])).out.includes('B:two'));
    assert.equal((await misao(daemon, ['send', 'send-keys', '--keys', 'NoSuchKey'])).code, 2);
    assert.equal((await misao(daemon, ['send', 'send-keys'])).code, 2, '送る内容が無い');
    assert.equal((await misao(daemon, ['send', 'send-keys', 'x', '--', 'y'])).code, 2, 'text と -- の後ろは同時に指定できない');
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

  test('new: origin=terminal を必ず付け、案内を 3 行出し、案内の短縮 ID で入れる', async () => {
    const r = await misao(daemon, ['new', '--label', 'origin=hub', '--label', 'owner=me', '--cwd', daemon.dir, '--', 'sh', '-c', 'echo $MISAO_T; sleep 60']);
    assert.equal(r.code, 0);
    const lines = r.out.trimEnd().split('\n');
    assert.equal(lines.length, 3, r.out);
    assert.match(lines[0]!, /^\[misao\] ペイン p_[0-9A-Z]{4}…[0-9A-Z]{2,} を作成しました（sh · /);
    assert.match(lines[1]!, /AZITO の Objects に「未登録」として表示されます/);
    const suffix = /入る: misao attach (\S+)$/.exec(lines[2]!)?.[1];
    assert.ok(suffix, lines[2]);
    const info = JSON.parse((await misao(daemon, ['label', suffix, 'x=1', '--json'])).out) as { labels: Record<string, string> };
    assert.deepEqual(info.labels, { origin: 'terminal', owner: 'me', x: '1' });
    await misao(daemon, ['kill', suffix, '--force']);
  });

  test('new --json: PaneInfo を返す。--env と --cwd が子に届く', async () => {
    const r = await misao(daemon, ['new', '--json', '--env', 'MISAO_T=from-env', '--cwd', daemon.dir, '--', 'sh', '-c', 'echo env:$MISAO_T; pwd; sleep 60']);
    assert.equal(r.code, 0);
    const pane = JSON.parse(r.out) as PaneInfo;
    assert.equal(pane.cwd, daemon.dir);
    assert.equal(pane.labels.origin, 'terminal');
    await waitFor(async () => {
      const screen = (await misao(daemon, ['screen', pane.paneId])).out;
      return screen.includes('env:from-env') && screen.includes(daemon.dir);
    });
    await misao(daemon, ['kill', pane.paneId, '--force']);
  });

  test('new: cmd 省略はログインシェル (-l)、シェルが取れなければ使い方の誤り', async () => {
    const r = await misao(daemon, ['new', '--json'], { shell: '/bin/sh' });
    const pane = JSON.parse(r.out) as PaneInfo;
    assert.deepEqual(pane.cmd, ['/bin/sh', '-l']);
    await misao(daemon, ['kill', pane.paneId, '--force']);
    const none = await misao(daemon, ['new'], { shell: null });
    assert.equal(none.code, 2);
  });

  test('new --workspace / --window: 無ければ作り、同じ指定なら同じ窓に置く', async () => {
    const make = async (): Promise<PaneInfo> =>
      JSON.parse((await misao(daemon, ['new', '--json', '--workspace', 'ws-a', '--window', 'win-a', '--', ...SLEEP])).out) as PaneInfo;
    const first = await make();
    const second = await make();
    assert.equal(first.workspace, 'ws-a');
    assert.equal(first.window.name, 'win-a');
    assert.equal(second.window.id, first.window.id);
    const onlyWindow = JSON.parse((await misao(daemon, ['new', '--json', '--window', 'solo', '--', ...SLEEP])).out) as PaneInfo;
    assert.equal(onlyWindow.workspace, 'default');
    const ls = JSON.parse((await misao(daemon, ['ls', '--json', '--workspace', 'ws-a'])).out) as PaneInfo[];
    assert.equal(ls.length, 2);
    await misao(daemon, ['kill', onlyWindow.paneId, '--force']);
    await misao(daemon, ['kill', first.paneId, '--workspace', '--force']);
    const left = JSON.parse((await misao(daemon, ['ls', '--json', '--workspace', 'ws-a'])).out) as PaneInfo[];
    assert.deepEqual(left, []);
  });

  test('kill: 確認なしで非 TTY は 2、n は中止 (1)、y と --force は閉じる', async () => {
    const paneId = await openTestPane(daemon.client, SLEEP, { labels: { name: 'kill-me' } });
    const exists = async (): Promise<boolean> =>
      ((await daemon.client.request('pane.list', {})).some((p) => p.paneId === paneId));

    assert.equal((await misao(daemon, ['kill', 'kill-me'])).code, 2);
    assert.equal(await exists(), true);

    const no = await misao(daemon, ['kill', 'kill-me'], { isTTY: true, stdin: 'n\n' });
    assert.equal(no.code, 1);
    assert.match(no.err, /\[y\/N\]/);
    assert.match(no.err, /中止しました/);
    assert.equal(await exists(), true);
    const eof = await misao(daemon, ['kill', 'kill-me'], { isTTY: true, stdin: '' });
    assert.equal(eof.code, 1, '入力が閉じたら中止');

    const yes = await misao(daemon, ['kill', 'kill-me'], { isTTY: true, stdin: 'y\n' });
    assert.equal(yes.code, 0, yes.err);
    assert.match(yes.err, /\(未登録\) kill-me/);
    assert.equal(await exists(), false);

    const forced = await openTestPane(daemon.client, SLEEP, { labels: { name: 'force-me' } });
    const f = await misao(daemon, ['kill', 'force-me', '--force', '--json']);
    assert.equal(f.code, 0);
    assert.deepEqual(JSON.parse(f.out), { closed: 'pane', paneIds: [forced] });
    assert.equal((await misao(daemon, ['kill', 'x', '--window', '--workspace', '--force'])).code, 2);
  });

  test('kill --window: 同じ窓のペインをまとめて閉じる', async () => {
    const mk = async (): Promise<PaneInfo> =>
      JSON.parse((await misao(daemon, ['new', '--json', '--window', 'doomed', '--workspace', 'ws-k', '--', ...SLEEP])).out) as PaneInfo;
    const a = await mk();
    const b = await mk();
    const r = await misao(daemon, ['kill', a.paneId, '--window', '--force', '--json']);
    assert.equal(r.code, 0, r.err);
    assert.deepEqual((JSON.parse(r.out) as { paneIds: string[] }).paneIds.sort(), [a.paneId, b.paneId].sort());
    const left = JSON.parse((await misao(daemon, ['ls', '--json', '--workspace', 'ws-k'])).out) as PaneInfo[];
    assert.deepEqual(left, []);
  });
});
