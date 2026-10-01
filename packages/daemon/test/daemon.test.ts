import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import xterm from '@xterm/headless';
import { PaneInfoSchema, methods, parseKnownEvent } from '@misao/protocol';
import type { EventParams, MethodName, PaneInfo } from '@misao/protocol';
import { Daemon } from '../src/daemon.js';
import { savePersistedState } from '../src/persistence.js';
import type { PersistedState } from '../src/persistence.js';
import { viewportText } from '../src/screen.js';
import { ulid } from '../src/ulid.js';
import { RpcClient, RpcClientError } from './helpers/rpc-client.js';
import { startDaemon, waitFor, withDaemon } from './helpers/daemon.js';

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function expectRpcError(p: Promise<unknown>, code: number): Promise<void> {
  await assert.rejects(p, (e: unknown) => e instanceof RpcClientError && e.code === code);
}

function openPane(client: RpcClient, cmd: string[], extra: Record<string, unknown> = {}): Promise<{ paneId: string }> {
  return client.request<{ paneId: string }>('pane.open', { cmd, ...extra });
}

test('daemon: pane.open → subscribe_lines でマーカーを観測 → 停止', async () => {
  let socketPath = '';
  await withDaemon(async (daemon, client) => {
    socketPath = daemon.socketPath;
    assert.equal(fs.statSync(socketPath).mode & 0o777, 0o600);
    assert.equal(fs.readFileSync(path.join(path.dirname(socketPath), 'daemon.pid'), 'utf8'), String(process.pid));

    const lines: Array<{ seq: number; text: string }> = [];
    const events: EventParams[] = [];
    client.onNotification((n) => {
      if (n.method === 'pane.line') lines.push({ seq: n.params.seq as number, text: n.params.text as string });
      if (n.method === 'event') events.push(n.params as EventParams);
    });
    await client.request('events.subscribe', { since: 0 });

    const { paneId } = await openPane(client, ['sh', '-c', 'printf "hello\\r\\nAZITO_DONE_x_1\\r\\n"; sleep 1'], {
      labels: { role: 'test' },
    });
    assert.match(paneId, /^p_[0-9A-Z]{26}$/);
    const res = await client.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since: 0 });
    assert.equal(res.gap, false);

    await waitFor(() => lines.some((l) => l.text === 'AZITO_DONE_x_1'));
    assert.ok(lines.some((l) => l.text === 'hello'));
    assert.deepEqual(lines.map((l) => l.seq), [...lines.map((l) => l.seq)].sort((a, b) => a - b));

    // 再購読 (since=0) でリングから 1..head が一度ずつ再生され、旧購読からの配信は重ならない
    const linesBefore = lines.length;
    const replayed: number[] = [];
    client.onNotification((n) => n.method === 'pane.line' && replayed.push(n.params.seq as number));
    const again = await client.request<{ head: number }>('pane.subscribe_lines', { paneId, since: 0 });
    assert.ok(again.head >= 2);
    await waitFor(() => replayed.length >= again.head);
    await sleep(100);
    assert.deepEqual(replayed, Array.from({ length: again.head }, (_, i) => i + 1));
    assert.equal(lines.length, linesBefore + again.head);

    await waitFor(() => events.some((e) => e.type === 'pane.exited'));
    const list = await client.request<PaneInfo[]>('pane.list');
    assert.equal(list.length, 1);
    assert.equal(list[0]!.processState, 'exited');
    assert.equal(list[0]!.agentState, 'exited');
    assert.equal(list[0]!.exitCode, 0);
    assert.deepEqual(list[0]!.labels, { role: 'test' });
    const types = events.map((e) => e.type);
    assert.ok(types.includes('daemon.started') && types.includes('pane.opened'));

    // 全イベントが既知スキーマに合い、pane に紐づくものは paneId が外側にある
    for (const ev of events) {
      const known = parseKnownEvent(ev);
      assert.ok(known, `known event: ${ev.type}`);
      assert.equal('paneId' in ev.data, false, `${ev.type}: paneId must not be in data`);
      assert.equal(ev.paneId === undefined, /^(daemon|workspace|window)\./.test(ev.type), `${ev.type}: paneId placement`);
    }

    await client.request('pane.close', { paneId });
    assert.equal((await client.request<PaneInfo[]>('pane.list')).length, 0);
  });
  assert.equal(fs.existsSync(socketPath), false);
});

test('daemon: write / screen / snapshot attach / env 除去', async () => {
  await withDaemon(async (daemon, client) => {
    process.env.TMUX = 'leaked-from-parent';
    try {
      const chunks: Buffer[] = [];
      client.onNotification((n) => {
        if (n.method === 'pane.output') chunks.push(Buffer.from(n.params.dataB64 as string, 'base64'));
      });
      const { paneId } = await openPane(client, [
        'sh',
        '-c',
        'echo "TMUX=[$TMUX] PANE=[$MISAO_PANE_ID] TERM=$TERM"; printf "\\033[31mred\\033[0m\\n"; exec cat',
      ]);
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
      const c2 = await RpcClient.connect(daemon.socketPath);
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
    }
  });
});

test('snapshot attach: 出力が流れ続ける pane で snapshot + live == pane.screen (境界の重複なし)', async () => {
  await withDaemon(async (daemon, client) => {
    // 出力の開始を read で制御し、出力が流れている最中に attach する
    const { paneId } = await openPane(client, [
      'sh',
      '-c',
      'echo ready; read go; i=0; while [ $i -lt 20000 ]; do printf "\\033[3$((i%7+1))mline $i\\033[0m\\r\\n"; i=$((i+1)); done; echo FINISHED; sleep 5',
    ]);
    const c2 = await RpcClient.connect(daemon.socketPath);
    const term = new xterm.Terminal({ cols: 80, rows: 24, scrollback: 100, allowProposedApi: true });
    const feed = (b: Buffer): Promise<void> => new Promise((r) => term.write(b, r));
    const parts: Buffer[] = [];
    const liveSeqs: number[] = [];
    let text = '';
    c2.onNotification((n) => {
      if (n.method !== 'pane.output') return;
      const b = Buffer.from(n.params.dataB64 as string, 'base64');
      parts.push(b);
      if (n.params.replay === undefined) liveSeqs.push(n.params.seq as number);
      text = (text + b.toString()).slice(-200);
    });
    await waitFor(async () => (await client.request<{ text: string }>('pane.screen', { paneId })).text.includes('ready'));
    // 書き込みの直後に attach する (出力の開始前でも最中でも、head より後は live で届く)
    await client.request('pane.write', { paneId, data: '\r' });
    const res = await c2.request<{ head: number }>('pane.attach', { paneId, clientId: 'snap', replay: 'snapshot' });
    assert.ok(res.head >= 1);
    await waitFor(() => text.includes('FINISHED'), 20000);
    // 境界をまたいでいる: head より後の live チャンクを受け取り、その seq は昇順で head を超える
    assert.ok(liveSeqs.length >= 1, 'received live chunks after the snapshot');
    assert.ok(liveSeqs.every((seq) => seq > res.head));
    assert.deepEqual(liveSeqs, [...liveSeqs].sort((a, b) => a - b));
    for (const p of parts) await feed(p);
    const expected = await client.request<{ text: string }>('pane.screen', { paneId });
    assert.equal(viewportText(term), expected.text);
    c2.close();
    await client.request('pane.close', { paneId });
  });
});

test('同じ (stream, pane) の再購読は置き換えで、since=lastSeq の再購読でも seq は連続・重複なし', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await openPane(client, ['sh', '-c', 'echo a; echo b; echo c; sleep 1; echo d; echo e; sleep 3']);
    const seqs: number[] = [];
    client.onNotification((n) => n.method === 'pane.line' && seqs.push(n.params.seq as number));
    await client.request('pane.subscribe_lines', { paneId, since: 0 });
    await waitFor(() => seqs.length >= 3);
    await client.request('pane.subscribe_lines', { paneId, since: seqs[seqs.length - 1]! });
    await client.request('pane.subscribe_lines', { paneId, since: seqs[seqs.length - 1]! });
    await waitFor(() => seqs.length >= 5);
    await sleep(200);
    assert.deepEqual(seqs, [1, 2, 3, 4, 5]);
    await client.request('pane.close', { paneId });
  });
});

test('サイズは最後に操作したクライアントが優先される', async () => {
  await withDaemon(async (daemon, client) => {
    const { paneId } = await openPane(client, ['sh', '-c', 'cat']);
    const info = async (): Promise<PaneInfo> => (await client.request<PaneInfo[]>('pane.list'))[0]!;
    const c2 = await RpcClient.connect(daemon.socketPath);
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

test('raw attach の結果に head / oldest / truncated が載る', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await openPane(client, ['sh', '-c', 'echo hi; sleep 3']);
    await sleep(300);
    const res = await client.request<{ head: number; oldest: number; truncated: boolean }>('pane.attach', {
      paneId,
      clientId: 'r',
      replay: 'raw',
    });
    assert.equal(res.oldest, 1);
    assert.equal(res.truncated, false);
    assert.ok(res.head >= 1);
    await client.request('pane.close', { paneId });
  });
});

test('改行なしで終了する最終行も行ストリームに流れる', async () => {
  await withDaemon(async (_daemon, client) => {
    const lines: string[] = [];
    client.onNotification((n) => n.method === 'pane.line' && lines.push(n.params.text as string));
    const { paneId } = await openPane(client, ['sh', '-c', 'sleep 0.3; printf AZITO_DONE_y_1']);
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
    const replies = got
      .join('')
      .split('\n')
      .filter(Boolean)
      .map((l) => JSON.parse(l) as { error: { code: number } });
    assert.deepEqual(replies.map((r) => r.error.code), [-32600, -32600, -32600, -32600, -32700]);
    raw.destroy();
    assert.equal((await client.request<{ pid: number }>('server.info')).pid, process.pid);
  });
});

test('epoch が現在と違う購読は gap: true で、保持している最古から再生する', async () => {
  await withDaemon(async (_daemon, client) => {
    const info = await client.request<{ epoch: string; eventHead: number }>('server.info');
    const seqs: number[] = [];
    client.onNotification((n) => n.method === 'event' && seqs.push(n.params.seq as number));
    const res = await client.request<{ gap: boolean; head: number; epoch: string }>('events.subscribe', {
      since: info.eventHead,
      epoch: ulid(),
    });
    assert.equal(res.gap, true);
    assert.equal(res.epoch, info.epoch);
    await waitFor(() => seqs.length >= 1);
    assert.equal(seqs[0], 1);

    // 同じ epoch なら gap なし
    const same = await client.request<{ gap: boolean }>('events.subscribe', { since: res.head, epoch: info.epoch });
    assert.equal(same.gap, false);
  });
});

test('エラー: cells は 1004、未実装メソッドは 1005、未知は -32601、params 違反は -32602、window.focus は未実装、window 不一致は 1007', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await openPane(client, ['sh', '-c', 'sleep 3']);
    await expectRpcError(client.request('pane.attach', { paneId, clientId: 'c', mode: 'cells' }), 1004);
    await expectRpcError(client.request('pane.respawn', { paneId }), 1005);
    await expectRpcError(client.request('pane.send_keys', { paneId, keys: ['Enter'] }), 1005);
    await expectRpcError(client.request('window.focus', { windowId: `w_${ulid()}`, clientId: 'c' }), 1005);
    await expectRpcError(client.request('pane.open', { cmd: ['true'], preplace: [] }), 1005);
    await expectRpcError(client.request('no.such'), -32601);
    await expectRpcError(client.request('pane.write', { paneId, data: 'a', dataB64: 'YQ==' }), -32602);
    await expectRpcError(client.request('pane.resize', { paneId, cols: 0, rows: 24 }), -32602);
    await expectRpcError(client.request('pane.list', { filter: { bogus: 1 } }), -32602);
    await expectRpcError(client.request('pane.info', { paneId: `p_${ulid()}` }), 1001);
    await expectRpcError(client.request('pane.open', { cmd: ['true'], windowId: `w_${ulid()}` }), 1007);
    await client.request('pane.close', { paneId });
  });
});

test('結果が protocol のスキーマに合い、pane.list の filter が効く', async () => {
  await withDaemon(async (_daemon, client) => {
    const a = await openPane(client, ['sh', '-c', 'sleep 3'], { labels: { owner: 'x' } });
    const b = await openPane(client, ['sh', '-c', 'exit 0'], { labels: { owner: 'y' } });
    const check = async (method: MethodName, params?: Record<string, unknown>): Promise<unknown> => {
      const result = await client.request(method, params);
      methods[method].result.parse(result);
      return result;
    };
    await check('server.info', {});
    await check('server.schema', {});
    await check('pane.info', { paneId: a.paneId });
    await check('pane.screen', { paneId: a.paneId });
    await check('pane.subscribe_lines', { paneId: a.paneId });
    const all = (await check('pane.list', {})) as PaneInfo[];
    for (const p of all) PaneInfoSchema.parse(p);
    assert.equal(all.length, 2);
    assert.deepEqual(all[0]!.window, all[1]!.window);
    assert.equal(all[0]!.workspace, 'default');

    const byLabel = (await check('pane.list', { filter: { labels: { owner: 'x' } } })) as PaneInfo[];
    assert.deepEqual(byLabel.map((p) => p.paneId), [a.paneId]);
    const byWorkspace = (await check('pane.list', { filter: { workspace: 'other' } })) as PaneInfo[];
    assert.deepEqual(byWorkspace, []);
    let exited: PaneInfo[] = [];
    for (let i = 0; i < 100 && exited.length === 0; i++) {
      exited = (await check('pane.list', { filter: { state: 'exited' } })) as PaneInfo[];
      if (exited.length === 0) await sleep(50);
    }
    assert.deepEqual(exited.map((p) => p.paneId), [b.paneId]);
    await client.request('pane.close', { paneId: a.paneId });
  });
});

test('同じ接続・同じ pane の attach が並行したら、置き換えられた側の snapshot は送られない', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await openPane(client, ['sh', '-c', 'echo ready; exec cat']);
    await sleep(200);
    const replays: unknown[] = [];
    client.onNotification((n) => n.method === 'pane.output' && n.params.replay !== undefined && replays.push(n.params.replay));
    const first = client.request('pane.attach', { paneId, clientId: 'A', replay: 'snapshot' });
    const second = client.request('pane.attach', { paneId, clientId: 'A', replay: 'none' });
    await Promise.all([first, second]);
    await sleep(200);
    assert.deepEqual(replays, []);
    await client.request('pane.close', { paneId });
  });
});

test('同じ pane への pane.close が並行しても pane.closed は 1 回だけ', async () => {
  await withDaemon(async (_daemon, client) => {
    const types: string[] = [];
    client.onNotification((n) => n.method === 'event' && types.push(n.params.type as string));
    await client.request('events.subscribe', {});
    const { paneId } = await openPane(client, ['sh', '-c', 'exec cat']);
    await Promise.all([client.request('pane.close', { paneId }), client.request('pane.close', { paneId })]);
    await sleep(100);
    assert.equal(types.filter((t) => t === 'pane.closed').length, 1);
  });
});

test('since: 0 の pane.subscribe_lines を同時に 2 本送っても、再生は置き換え後の 1 本だけで seq は重複しない', async () => {
  await withDaemon(async (_daemon, client) => {
    const types: string[] = [];
    client.onNotification((n) => n.method === 'event' && types.push(n.params.type as string));
    await client.request('events.subscribe', {});
    const { paneId } = await openPane(client, ['sh', '-c', 'echo a; echo b; echo c']);
    await waitFor(() => types.includes('pane.exited'));
    const seqs: number[] = [];
    client.onNotification((n) => n.method === 'pane.line' && seqs.push(n.params.seq as number));
    const [r1, r2] = await Promise.all([
      client.request<{ head: number }>('pane.subscribe_lines', { paneId, since: 0 }),
      client.request<{ head: number }>('pane.subscribe_lines', { paneId, since: 0 }),
    ]);
    assert.equal(r1.head, 3);
    assert.equal(r2.head, 3);
    await waitFor(() => seqs.length >= 3);
    await sleep(100);
    assert.deepEqual(seqs, [1, 2, 3]);
    await client.request('pane.close', { paneId });
  });
});

test('pane.close は全接続の attach と行購読を外し、client.detached は pane.closed より前にだけ出る', async () => {
  await withDaemon(async (daemon, client) => {
    const events: EventParams[] = [];
    client.onNotification((n) => n.method === 'event' && events.push(n.params as EventParams));
    await client.request('events.subscribe', {});
    const { paneId } = await openPane(client, ['sh', '-c', 'exec cat']);
    const c2 = await RpcClient.connect(daemon.socketPath);
    const c2Lines: number[] = [];
    c2.onNotification((n) => n.method === 'pane.line' && c2Lines.push(n.params.seq as number));
    await c2.request('pane.attach', { paneId, clientId: 'B', replay: 'none' });
    await c2.request('pane.subscribe_lines', { paneId });

    await client.request('pane.close', { paneId });
    // 接続を閉じても、閉じた pane についての client.detached は追加で出ない
    c2.close();
    await sleep(200);
    const forPane = events.filter((e) => e.paneId === paneId).map((e) => e.type);
    assert.deepEqual(forPane.slice(-2), ['client.detached', 'pane.closed']);
    assert.equal(forPane.filter((t) => t === 'client.detached').length, 1);
    assert.deepEqual(c2Lines, []);
  });
});

test('同じ接続・同じ clientId の再 attach ではサイズの所有権と記録を保ち、client.detached を出さない', async () => {
  await withDaemon(async (daemon, client) => {
    const types: string[] = [];
    client.onNotification((n) => n.method === 'event' && types.push(n.params.type as string));
    await client.request('events.subscribe', {});
    const { paneId } = await openPane(client, ['sh', '-c', 'exec cat']);
    const info = async (): Promise<PaneInfo> => client.request<PaneInfo>('pane.info', { paneId });
    const c2 = await RpcClient.connect(daemon.socketPath);
    await client.request('pane.attach', { paneId, clientId: 'A', replay: 'none', cols: 100, rows: 30 });
    await c2.request('pane.attach', { paneId, clientId: 'B', replay: 'none', cols: 60, rows: 20 });
    // B がサイズ指定なしで再 attach (replay を変えて再同期) しても、所有者は B のまま
    await c2.request('pane.attach', { paneId, clientId: 'B', replay: 'snapshot' });
    let i = await info();
    assert.deepEqual([i.cols, i.rows, i.sizeOwner], [60, 20, 'B']);
    assert.deepEqual([...i.clients].sort(), ['A', 'B']);
    // A が操作して所有権を取った後、B の write で B のサイズに戻せる (記録が残っている)
    await client.request('pane.write', { paneId, data: 'x', clientId: 'A' });
    assert.equal((await info()).sizeOwner, 'A');
    await c2.request('pane.write', { paneId, data: 'y', clientId: 'B' });
    i = await info();
    assert.deepEqual([i.cols, i.rows, i.sizeOwner], [60, 20, 'B']);
    await sleep(50);
    assert.equal(types.filter((t) => t === 'client.detached').length, 0);
    assert.equal(types.filter((t) => t === 'client.attached').length, 2);
    c2.close();
    await client.request('pane.close', { paneId });
  });
});

test('同じソケットと pid ファイルで 2 つ目のデーモンは起動に失敗し、1 つ目は使い続けられる', async () => {
  await withDaemon(async (daemon, client) => {
    const dir = path.dirname(daemon.socketPath);
    const second = new Daemon({
      socketPath: daemon.socketPath,
      pidPath: path.join(dir, 'daemon.pid'),
      statePath: path.join(dir, 'persistence.json'),
      log: () => undefined,
    });
    await assert.rejects(second.start(), /another daemon/);
    assert.ok(fs.existsSync(daemon.socketPath));
    assert.equal(fs.readFileSync(path.join(dir, 'daemon.pid'), 'utf8'), String(process.pid));
    assert.equal((await client.request<{ pid: number }>('server.info')).pid, process.pid);
  });
});

test('起動に失敗したら pid ファイルを残さない', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  try {
    const socketPath = path.join(dir, 'misao.sock');
    const pidPath = path.join(dir, 'daemon.pid');
    fs.mkdirSync(socketPath); // ソケットの場所にディレクトリ: 古いソケットとは断定できない
    const daemon = new Daemon({ socketPath, pidPath, statePath: path.join(dir, 'persistence.json'), log: () => undefined });
    await assert.rejects(daemon.start());
    assert.equal(fs.existsSync(pidPath), false);
    assert.ok(fs.statSync(socketPath).isDirectory());
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

async function collectEvents(client: RpcClient): Promise<EventParams[]> {
  const events: EventParams[] = [];
  client.onNotification((n) => n.method === 'event' && events.push(n.params as EventParams));
  await client.request('events.subscribe', { since: 0 });
  return events;
}

const eventTypes = (events: EventParams[]): string[] => events.map((e) => e.type);

/** 一時ディレクトリで、1 つ目のデーモンを止めて 2 つ目を起動するテスト用。 */
async function withRestart(
  fn: (restart: () => Promise<RpcClient>, first: RpcClient, dir: string) => Promise<void>,
): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  const daemons: Daemon[] = [];
  const clients: RpcClient[] = [];
  const boot = async (): Promise<RpcClient> => {
    const daemon = await startDaemon(dir);
    daemons.push(daemon);
    const client = await RpcClient.connect(daemon.socketPath);
    clients.push(client);
    return client;
  };
  try {
    const first = await boot();
    await fn(
      async () => {
        clients.at(-1)!.close();
        await daemons.at(-1)!.shutdown();
        return boot();
      },
      first,
      dir,
    );
  } finally {
    for (const c of clients) c.close();
    await daemons.at(-1)!.shutdown();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test('pane.set_label: set / unset を適用した labels 全体を返し、pane.label を出す', async () => {
  await withDaemon(async (_daemon, client) => {
    const events = await collectEvents(client);
    const { paneId } = await openPane(client, ['sh', '-c', 'sleep 3'], { labels: { a: '1', b: '2' } });
    const res = await client.request<{ labels: Record<string, string> }>('pane.set_label', {
      paneId,
      set: { b: '3', c: '4' },
      unset: ['a'],
    });
    assert.deepEqual(res.labels, { b: '3', c: '4' });
    assert.deepEqual((await client.request<PaneInfo>('pane.info', { paneId })).labels, res.labels);
    await client.request('pane.set_label', { paneId, unset: ['c'] });
    await waitFor(() => events.filter((e) => e.type === 'pane.label').length === 2);
    const [first, second] = events.filter((e) => e.type === 'pane.label');
    assert.deepEqual(first!.data, { set: { b: '3', c: '4' }, unset: ['a'] });
    assert.deepEqual(second!.data, { set: {}, unset: ['c'] });
    assert.equal(first!.paneId, paneId);
    await expectRpcError(client.request('pane.set_label', { paneId: `p_${ulid()}`, unset: ['a'] }), 1001);
    await client.request('pane.close', { paneId });
  });
});

test('labels: { origin: "terminal" } で未登録ペインを絞り込める', async () => {
  await withDaemon(async (_daemon, client) => {
    const a = await openPane(client, ['sh', '-c', 'sleep 3'], { labels: { origin: 'terminal' } });
    const b = await openPane(client, ['sh', '-c', 'sleep 3'], { labels: { origin: 'hub' } });
    const found = await client.request<PaneInfo[]>('pane.list', { filter: { labels: { origin: 'terminal' } } });
    assert.deepEqual(found.map((p) => p.paneId), [a.paneId]);
    await Promise.all([a, b].map((p) => client.request('pane.close', { paneId: p.paneId })));
  });
});

test('workspace / window の RPC とイベント、workspace フィルタ', async () => {
  await withDaemon(async (_daemon, client) => {
    const events = await collectEvents(client);
    const ws = await client.request<{ name: string; windows: unknown[] }>('workspace.create', { name: 'proj' });
    assert.deepEqual(ws, { name: 'proj', windows: [] });
    await expectRpcError(client.request('workspace.create', { name: 'proj' }), 1008);
    const win = await client.request<{ windowId: string; name: string; workspace: string }>('window.create', {
      workspace: 'proj',
      name: 'main',
    });
    await expectRpcError(client.request('window.create', { workspace: 'none', name: 'x' }), 1006);
    const inProj = await openPane(client, ['sh', '-c', 'sleep 5'], { windowId: win.windowId });
    const inDefault = await openPane(client, ['sh', '-c', 'sleep 5']);
    const info = await client.request<PaneInfo>('pane.info', { paneId: inProj.paneId });
    assert.equal(info.workspace, 'proj');
    assert.deepEqual(info.window, { id: win.windowId, name: 'main' });
    const byWs = await client.request<PaneInfo[]>('pane.list', { filter: { workspace: 'proj' } });
    assert.deepEqual(byWs.map((p) => p.paneId), [inProj.paneId]);
    const byDefault = await client.request<PaneInfo[]>('pane.list', { filter: { workspace: 'default' } });
    assert.deepEqual(byDefault.map((p) => p.paneId), [inDefault.paneId]);

    await client.request('window.rename', { windowId: win.windowId, name: 'renamed' });
    await client.request('workspace.rename', { name: 'proj', newName: 'proj2' });
    const list = await client.request<Array<{ name: string; windows: Array<{ windowId: string; name: string }> }>>('workspace.list');
    assert.deepEqual(list.map((w) => w.name), ['proj2', 'default']);
    assert.deepEqual(list[0]!.windows.map((w) => w.name), ['renamed']);

    // workspace を閉じると配下の pane が閉じ、pane.closed は window.closed / workspace.closed より前に出る
    await client.request('workspace.close', { name: 'proj2' });
    await expectRpcError(client.request('pane.info', { paneId: inProj.paneId }), 1001);
    await expectRpcError(client.request('window.close', { windowId: win.windowId }), 1007);
    const types = eventTypes(events);
    assert.ok(types.indexOf('pane.closed') < types.indexOf('window.closed'));
    assert.ok(types.indexOf('window.closed') < types.indexOf('workspace.closed'));
    const names = ['workspace.created', 'window.created', 'window.renamed', 'workspace.renamed'];
    for (const n of names) assert.ok(types.includes(n), n);
    for (const ev of events) assert.ok(parseKnownEvent(ev), ev.type);

    // window.close も配下の pane を閉じる
    const second = await client.request<{ windowId: string }>('window.create', { workspace: 'default', name: 'second' });
    const p = await openPane(client, ['sh', '-c', 'sleep 5'], { windowId: second.windowId });
    await client.request('window.close', { windowId: second.windowId });
    await expectRpcError(client.request('pane.info', { paneId: p.paneId }), 1001);
    await client.request('pane.close', { paneId: inDefault.paneId });
  });
});

test('デーモン再起動後、pane は stopped で ID・cmd・cwd・labels・window を保つ', async () => {
  await withRestart(async (restart, first) => {
    await first.request('workspace.create', { name: 'work' });
    const win = await first.request<{ windowId: string }>('window.create', { workspace: 'work', name: 'x' });
    const { paneId } = await openPane(first, ['sh', '-c', 'sleep 30'], {
      cwd: os.tmpdir(),
      labels: { owner: 'me' },
      cols: 100,
      rows: 30,
      windowId: win.windowId,
    });
    const before = await first.request<PaneInfo>('pane.info', { paneId });
    assert.equal(before.processState, 'running');

    const client = await restart();
    const after = await client.request<PaneInfo>('pane.info', { paneId });
    PaneInfoSchema.parse(after);
    assert.equal(after.processState, 'stopped');
    assert.equal(after.pid, null);
    assert.deepEqual(after.cmd, ['sh', '-c', 'sleep 30']);
    assert.equal(after.cwd, os.tmpdir());
    assert.deepEqual(after.labels, { owner: 'me' });
    assert.deepEqual([after.cols, after.rows], [100, 30]);
    assert.deepEqual(after.window, before.window);
    const listed = await client.request<PaneInfo[]>('pane.list', { filter: { state: 'stopped' } });
    assert.deepEqual(listed.map((p) => p.paneId), [paneId]);
    assert.equal((await client.request<PaneInfo[]>('pane.list', { filter: { state: 'running' } })).length, 0);
    assert.equal((await client.request<{ paneCount: number }>('server.info')).paneCount, 1);
  });
});

test('stopped の pane への write / resize / screen / attach / subscribe_lines は PaneExited、close は pane.closed を出す', async () => {
  await withRestart(async (restart, first) => {
    const { paneId } = await openPane(first, ['sh', '-c', 'sleep 30']);
    const client = await restart();
    const events = await collectEvents(client);
    await expectRpcError(client.request('pane.write', { paneId, data: 'x' }), 1002);
    await expectRpcError(client.request('pane.resize', { paneId, cols: 80, rows: 24 }), 1002);
    await expectRpcError(client.request('pane.screen', { paneId }), 1002);
    await expectRpcError(client.request('pane.attach', { paneId, clientId: 'c' }), 1002);
    await expectRpcError(client.request('pane.subscribe_lines', { paneId }), 1002);
    await client.request('pane.close', { paneId });
    await expectRpcError(client.request('pane.info', { paneId }), 1001);
    assert.deepEqual(events.filter((e) => e.type === 'pane.closed').map((e) => e.paneId), [paneId]);
    assert.equal((await client.request<PaneInfo[]>('pane.list')).length, 0);
    // 閉じたことが保存されている
    const again = await restart();
    assert.equal((await again.request<PaneInfo[]>('pane.list')).length, 0);
  });
});

test('pane.open の既定 workspace / window は再起動後も同じ ID で再利用される', async () => {
  await withRestart(async (restart, first) => {
    const { paneId } = await openPane(first, ['sh', '-c', 'sleep 30']);
    const before = await first.request<PaneInfo>('pane.info', { paneId });
    const client = await restart();
    const second = await openPane(client, ['sh', '-c', 'sleep 30']);
    const after = await client.request<PaneInfo>('pane.info', { paneId: second.paneId });
    assert.deepEqual(after.window, before.window);
    assert.equal(after.workspace, 'default');
    await client.request('pane.close', { paneId: second.paneId });
  });
});

test('壊れた persistence.json ではデーモンが起動せず、pid ファイルを残さない', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  try {
    const statePath = path.join(dir, 'persistence.json');
    fs.writeFileSync(statePath, '{broken');
    await assert.rejects(startDaemon(dir), (e: unknown) => e instanceof Error && e.message.includes(statePath));
    assert.equal(fs.existsSync(path.join(dir, 'daemon.pid')), false);
    assert.equal(fs.existsSync(path.join(dir, 'misao.sock')), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('ephemeralEnv は子の環境にだけ渡り、保存・応答・イベントのどこにも出ない', async () => {
  await withRestart(async (restart, first, dir) => {
    const events = await collectEvents(first);
    const lines: string[] = [];
    first.onNotification((n) => n.method === 'pane.line' && lines.push(n.params.text as string));
    const { paneId } = await openPane(first, ['sh', '-c', 'printf "%s:%s\\r\\n" "$TOKEN" "$SHARED"; sleep 5'], {
      env: { SHARED: 'from-env', PLAIN: 'p' },
      ephemeralEnv: { TOKEN: 'secret-xyz', SHARED: 'from-ephemeral' },
    });
    await first.request('pane.subscribe_lines', { paneId, since: 0 });
    await waitFor(() => lines.includes('secret-xyz:from-ephemeral'));

    const saved = fs.readFileSync(path.join(dir, 'persistence.json'), 'utf8');
    // cmd 自体が "$TOKEN" を含むので、キー名 TOKEN は生テキストではなく保存された env で確認する
    for (const secret of ['secret-xyz', 'from-ephemeral']) assert.equal(saved.includes(secret), false, secret);
    const persisted = JSON.parse(saved) as { panes: Array<{ env: Record<string, string> }> };
    assert.deepEqual(persisted.panes[0]!.env, { SHARED: 'from-env', PLAIN: 'p' });

    const exposed = JSON.stringify([
      await first.request('pane.info', { paneId }),
      await first.request('pane.list'),
      events,
    ]);
    assert.equal(exposed.includes('secret-xyz'), false);
    assert.ok(eventTypes(events).includes('pane.opened'));

    const client = await restart();
    assert.equal(JSON.stringify(await client.request('pane.info', { paneId })).includes('secret-xyz'), false);
  });
});

test('保存に失敗したら pane.open / workspace.create / pane.set_label は戻り、イベントも出ない', async () => {
  await withDaemon(async (daemon, client) => {
    const events = await collectEvents(client);
    const keep = await openPane(client, ['sh', '-c', 'sleep 5'], { labels: { a: '1' } });
    const before = await client.request('workspace.list');
    const eventCount = events.length;
    // statePath の位置にディレクトリがあると、保存 (rename) は root でも失敗する
    const statePath = path.join(path.dirname(daemon.socketPath), 'persistence.json');
    fs.rmSync(statePath);
    fs.mkdirSync(statePath);

    await assert.rejects(openPane(client, ['sh', '-c', 'sleep 5']), RpcClientError);
    await assert.rejects(client.request('workspace.create', { name: 'x' }), RpcClientError);
    await assert.rejects(client.request('window.create', { workspace: 'default', name: 'x' }), RpcClientError);
    await assert.rejects(client.request('pane.set_label', { paneId: keep.paneId, set: { a: '2', b: '3' } }), RpcClientError);

    assert.deepEqual((await client.request<PaneInfo[]>('pane.list')).map((p) => p.paneId), [keep.paneId]);
    assert.deepEqual(await client.request('workspace.list'), before);
    assert.deepEqual((await client.request<PaneInfo>('pane.info', { paneId: keep.paneId })).labels, { a: '1' });
    await sleep(100);
    assert.equal(events.length, eventCount);
    fs.rmdirSync(statePath);
    await client.request('pane.close', { paneId: keep.paneId });
  });
});

test('既定の workspace / window の遅延作成も、保存に失敗したら戻る', async () => {
  await withDaemon(async (daemon, client) => {
    fs.mkdirSync(path.join(path.dirname(daemon.socketPath), 'persistence.json'));
    await assert.rejects(openPane(client, ['sh', '-c', 'sleep 5']), RpcClientError);
    assert.deepEqual(await client.request('workspace.list'), []);
    assert.deepEqual(await client.request('pane.list'), []);
  });
});

test('resize で変えたサイズが保存され、再起動後の stopped に反映される', async () => {
  await withRestart(async (restart, first) => {
    const { paneId } = await openPane(first, ['sh', '-c', 'sleep 30']);
    await first.request('pane.resize', { paneId, cols: 111, rows: 33 });
    const client = await restart();
    const info = await client.request<PaneInfo>('pane.info', { paneId });
    assert.deepEqual([info.cols, info.rows], [111, 33]);
  });
});

/** 後から保存を失敗させられる saveState。 */
function failableSave(): { fail: { on: boolean }; saveState: (p: string, s: PersistedState) => void; calls: { n: number } } {
  const fail = { on: false };
  const calls = { n: 0 };
  return {
    fail,
    calls,
    saveState: (p, s) => {
      calls.n++;
      if (fail.on) throw new Error('disk full');
      savePersistedState(p, s);
    },
  };
}

async function withTempDir(fn: (dir: string) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  try {
    await fn(dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

async function expectRpcMessage(p: Promise<unknown>, code: number, message: RegExp): Promise<void> {
  await assert.rejects(p, (e: unknown) => e instanceof RpcClientError && e.code === code && message.test(e.message));
}

/** SIGHUP を無視する pane。trap の設定が済んでから返すので、閉じるのに SIGKILL (約 3 秒) までかかる。 */
async function openStubborn(client: RpcClient, windowId: string): Promise<{ paneId: string }> {
  const lines: string[] = [];
  client.onNotification((n) => n.method === 'pane.line' && lines.push(n.params.text as string));
  const pane = await openPane(client, ['sh', '-c', 'trap "" HUP; echo ready; sleep 30'], { windowId });
  await client.request('pane.subscribe_lines', { paneId: pane.paneId, since: 0 });
  await waitFor(() => lines.includes('ready'));
  return pane;
}

test('workspace.close の待機中は配下への window.create / pane.open / rename を拒否し、孤児 pane を作らない', async () => {
  await withRestart(async (restart, client) => {
    await client.request('workspace.create', { name: 'proj' });
    const win = await client.request<{ windowId: string }>('window.create', { workspace: 'proj', name: 'main' });
    await openStubborn(client, win.windowId);
    const closing = client.request('workspace.close', { name: 'proj' });
    await expectRpcMessage(client.request('window.create', { workspace: 'proj', name: 'late' }), 1006, /closing/);
    await expectRpcMessage(client.request('pane.open', { cmd: ['true'], windowId: win.windowId }), 1007, /closing/);
    await expectRpcMessage(client.request('window.close', { windowId: win.windowId }), 1007, /closing/);
    await expectRpcMessage(client.request('workspace.close', { name: 'proj' }), 1006, /closing/);
    // rename で名前を空け、同名を作り直して無関係な workspace を消させる、ことはできない
    await expectRpcMessage(client.request('workspace.rename', { name: 'proj', newName: 'other' }), 1006, /closing/);
    await expectRpcError(client.request('workspace.create', { name: 'proj' }), 1008);
    await closing;
    assert.deepEqual(await client.request('pane.list'), []);
    assert.deepEqual(await client.request('workspace.list'), []);
    const after = await restart(); // 孤児が無いので、保存したファイルで再起動できる
    assert.deepEqual(await after.request('pane.list'), []);
  });
});

test('window.close の待機中の workspace.close は、closing window を理由に WorkspaceNotFound で拒否する', async () => {
  await withRestart(async (_restart, client) => {
    await client.request('workspace.create', { name: 'proj' });
    const win = await client.request<{ windowId: string }>('window.create', { workspace: 'proj', name: 'main' });
    await openStubborn(client, win.windowId);
    const closing = client.request('window.close', { windowId: win.windowId });
    await expectRpcMessage(client.request('workspace.close', { name: 'proj' }), 1006, /closing window/);
    await closing;
  });
});

test('所有者の detach で継承したサイズは、shutdown 後の再起動で復元される', async () => {
  await withTempDir(async (dir) => {
    const first = await startDaemon(dir);
    const c1 = await RpcClient.connect(first.socketPath);
    const c2 = await RpcClient.connect(first.socketPath);
    const { paneId } = await openPane(c1, ['sh', '-c', 'sleep 30']);
    await c1.request('pane.attach', { paneId, clientId: 'A', replay: 'none', cols: 100, rows: 30 });
    await c2.request('pane.attach', { paneId, clientId: 'B', replay: 'none', cols: 60, rows: 20 });
    await c2.request('pane.detach', { paneId }); // 所有者 B が離脱し、A のサイズ (100x30) を継承する
    c1.close();
    c2.close();
    await first.shutdown();

    const second = await startDaemon(dir);
    const client = await RpcClient.connect(second.socketPath);
    try {
      const info = await client.request<PaneInfo>('pane.info', { paneId });
      assert.deepEqual([info.cols, info.rows], [100, 30]);
    } finally {
      client.close();
      await second.shutdown();
    }
  });
});

test('window.close の待機中は、その window への pane.open を拒否する', async () => {
  await withRestart(async (restart, client) => {
    await client.request('workspace.create', { name: 'proj' });
    const win = await client.request<{ windowId: string }>('window.create', { workspace: 'proj', name: 'main' });
    await openStubborn(client, win.windowId);
    const closing = client.request('window.close', { windowId: win.windowId });
    await expectRpcMessage(client.request('pane.open', { cmd: ['true'], windowId: win.windowId }), 1007, /closing/);
    await closing;
    const after = await restart();
    assert.deepEqual(await after.request('pane.list'), []);
  });
});

test('戻せない close は、保存に失敗しても closed 系イベントを出してから例外を返す', async () => {
  await withTempDir(async (dir) => {
    const { fail, saveState } = failableSave();
    const daemon = await startDaemon(dir, { saveState });
    const client = await RpcClient.connect(daemon.socketPath);
    try {
      const events = await collectEvents(client);
      const live = await openPane(client, ['sh', '-c', 'sleep 5']);
      await client.request('workspace.create', { name: 'proj' });
      const win = await client.request<{ windowId: string }>('window.create', { workspace: 'proj', name: 'w' });
      const inWin = await openPane(client, ['sh', '-c', 'sleep 5'], { windowId: win.windowId });
      fail.on = true;

      await assert.rejects(client.request('pane.close', { paneId: live.paneId }), RpcClientError);
      await assert.rejects(client.request('window.close', { windowId: win.windowId }), RpcClientError);
      await assert.rejects(client.request('workspace.close', { name: 'proj' }), RpcClientError);
      await waitFor(() => eventTypes(events).includes('workspace.closed'));
      const closed = events.filter((e) => e.type === 'pane.closed').map((e) => e.paneId);
      assert.deepEqual(closed.sort(), [live.paneId, inWin.paneId].sort());
      assert.ok(eventTypes(events).includes('window.closed'));
      // メモリは実態 (閉じ済み) に合っている
      assert.deepEqual(await client.request('pane.list'), []);
      assert.deepEqual((await client.request<Array<{ name: string }>>('workspace.list')).map((w) => w.name), ['default']);
    } finally {
      client.close();
      await daemon.shutdown();
    }
  });
});

test('戻せる stopped pane の close は、保存に失敗したら元に戻り pane.closed も出ない', async () => {
  await withTempDir(async (dir) => {
    const first = await startDaemon(dir);
    const c1 = await RpcClient.connect(first.socketPath);
    const { paneId } = await openPane(c1, ['sh', '-c', 'sleep 30']);
    c1.close();
    await first.shutdown();

    const { fail, saveState } = failableSave();
    const daemon = await startDaemon(dir, { saveState });
    const client = await RpcClient.connect(daemon.socketPath);
    try {
      const events = await collectEvents(client);
      fail.on = true;
      await assert.rejects(client.request('pane.close', { paneId }), RpcClientError);
      assert.equal((await client.request<PaneInfo>('pane.info', { paneId })).processState, 'stopped');
      await sleep(50);
      assert.equal(eventTypes(events).includes('pane.closed'), false);
    } finally {
      client.close();
      await daemon.shutdown();
    }
  });
});

test('サイズ変更は保存を遅らせるだけで、保存が失敗しても resize / write / attach は成功する', async () => {
  await withTempDir(async (dir) => {
    const { fail, saveState, calls } = failableSave();
    const logs: string[] = [];
    const daemon = await startDaemon(dir, { saveState, log: (m) => logs.push(m) });
    const client = await RpcClient.connect(daemon.socketPath);
    try {
      const { paneId } = await openPane(client, ['sh', '-c', 'exec cat']);
      fail.on = true;
      const callsBefore = calls.n;
      await client.request('pane.resize', { paneId, cols: 100, rows: 30, clientId: 'a' });
      await client.request('pane.resize', { paneId, cols: 90, rows: 20, clientId: 'b' });
      for (let i = 0; i < 20; i++) await client.request('pane.resize', { paneId, cols: 100 + i, rows: 30, clientId: 'a' });
      await client.request('pane.write', { paneId, data: 'x', clientId: 'b' }); // b がサイズを取り戻す
      await client.request('pane.attach', { paneId, clientId: 'c', cols: 70, rows: 25 });
      assert.equal(calls.n, callsBefore, 'サイズ変更では同期的に保存しない');
    } finally {
      client.close();
      await daemon.shutdown(); // 遅らせていた保存を行い、失敗はログだけ
    }
    assert.ok(logs.some((m) => m.includes('failed to persist state: disk full')), logs.join('|'));
  });
});

test('サイズ変更の遅らせた保存は shutdown でまとめて 1 回行われる', async () => {
  await withTempDir(async (dir) => {
    const { saveState, calls } = failableSave();
    const daemon = await startDaemon(dir, { saveState });
    const client = await RpcClient.connect(daemon.socketPath);
    const { paneId } = await openPane(client, ['sh', '-c', 'sleep 30']);
    const callsBefore = calls.n;
    for (let i = 0; i < 5; i++) await client.request('pane.resize', { paneId, cols: 100 + i, rows: 30 });
    client.close();
    await daemon.shutdown();
    assert.equal(calls.n, callsBefore + 1);
  });
});
