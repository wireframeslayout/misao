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
import { viewportText } from '../src/screen.js';
import { ulid } from '../src/ulid.js';
import { RpcClient, RpcClientError } from './helpers/rpc-client.js';
import { waitFor, withDaemon } from './helpers/daemon.js';

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
      assert.equal(ev.paneId === undefined, ev.type === 'daemon.started', `${ev.type}: paneId placement`);
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

test('エラー: cells は 1004、未実装メソッドは 1005、未知は -32601、params 違反は -32602、window 不一致は 1007', async () => {
  await withDaemon(async (_daemon, client) => {
    const { paneId } = await openPane(client, ['sh', '-c', 'sleep 3']);
    await expectRpcError(client.request('pane.attach', { paneId, clientId: 'c', mode: 'cells' }), 1004);
    await expectRpcError(client.request('pane.respawn', { paneId }), 1005);
    await expectRpcError(client.request('pane.send_keys', { paneId, keys: ['Enter'] }), 1005);
    await expectRpcError(client.request('workspace.list'), 1005);
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
    const daemon = new Daemon({ socketPath, pidPath, log: () => undefined });
    await assert.rejects(daemon.start());
    assert.equal(fs.existsSync(pidPath), false);
    assert.ok(fs.statSync(socketPath).isDirectory());
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
