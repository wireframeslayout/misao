import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import * as net from 'node:net';
import { Connection } from '../src/connection.js';
import { SeqRing } from '../src/ring.js';
import { subscribeStream } from '../src/stream.js';
import type { RequestContext } from '../src/stream.js';

const TS = '2026-01-01T00:00:00.000Z';
const EPOCH = 'epoch-1';

const server = net.createServer();
let sockPath = '';

before(async () => {
  sockPath = `/tmp/misao-stream-${process.pid}-${Date.now()}.sock`;
  await new Promise<void>((r) => server.listen(sockPath, r));
});
after(async () => {
  await new Promise<void>((r) => server.close(() => r()));
});

interface Harness {
  conn: Connection;
  client: net.Socket;
  ctx: RequestContext;
  /** afterReply に積まれた処理を実行する (= 応答を送った後)。 */
  reply(): void;
  close(): Promise<void>;
}

async function harness(): Promise<Harness> {
  let resolveClosed!: () => void;
  const closed = new Promise<void>((r) => (resolveClosed = r));
  const accepted = new Promise<Connection>((resolve) => {
    server.once('connection', (socket) => resolve(new Connection(socket, { onMessage: () => undefined, onClose: () => resolveClosed() })));
  });
  const client = net.connect(sockPath);
  client.on('error', () => undefined);
  const conn = await accepted;
  const after: Array<() => void> = [];
  return {
    conn,
    client,
    ctx: { conn, afterReply: (fn) => after.push(fn) },
    reply: () => after.splice(0).forEach((fn) => fn()),
    close: async () => {
      client.destroy();
      await closed;
    },
  };
}

/** 本物の SeqRing に push して、購読者へ「新しい要素」を知らせる最小の発行元。 */
function source<T>(capacity: number) {
  const ring = new SeqRing<T>(capacity);
  const listeners = new Set<() => void>();
  return {
    ring,
    on: (cb: () => void): (() => void) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    push(item: T, size = 1): number {
      const seq = ring.push(item, size, TS);
      for (const cb of [...listeners]) cb();
      return seq;
    },
    get listenerCount(): number {
      return listeners.size;
    },
  };
}

function subscribeRecording(h: Harness, src: ReturnType<typeof source<string>>, since: number | undefined, extra: { key?: string; epoch?: string } = {}) {
  const seqs: number[] = [];
  const result = subscribeStream({
    ctx: h.ctx,
    key: extra.key ?? 'k',
    ring: src.ring,
    on: src.on,
    notify: (seq) => seqs.push(seq),
    since,
    epoch: extra.epoch,
    currentEpoch: EPOCH,
  });
  return { seqs, result };
}

function subscribeToSocket(h: Harness, src: ReturnType<typeof source<string>>, since: number | undefined) {
  return subscribeStream({
    ctx: h.ctx,
    key: 'k',
    ring: src.ring,
    on: src.on,
    notify: (seq, item, ts) => h.conn.notify('event', seq, ts, { item }),
    since,
    epoch: undefined,
    currentEpoch: EPOCH,
  });
}

async function readSeqs(client: net.Socket, count: number): Promise<number[]> {
  const seqs: number[] = [];
  let buf = '';
  await new Promise<void>((resolve, reject) => {
    client.on('data', (chunk: Buffer) => {
      buf += chunk.toString();
      const lines = buf.split('\n');
      buf = lines.pop()!;
      for (const line of lines) seqs.push((JSON.parse(line) as { params: { seq: number } }).params.seq);
      if (seqs.length >= count) resolve();
    });
    client.once('close', () => reject(new Error(`closed after ${seqs.length} of ${count}`)));
    client.resume();
  });
  return seqs;
}

test('stream: pause したクライアントは送信量が 16 MiB を超えてもリング内なら切られず、resume 後に seq 連続で全件届く', async () => {
  const h = await harness();
  const payload = 'x'.repeat(1024 * 1024);
  const total = 40; // 40 MiB > MAX_WRITABLE_BYTES
  const src = source<string>(64 * 1024 * 1024);
  subscribeToSocket(h, src, undefined);
  h.reply();
  h.client.pause();
  for (let i = 0; i < total; i++) src.push(payload, payload.length);
  assert.equal(h.conn.socket.destroyed, false);
  assert.ok(h.conn.socket.writableLength < 4 * 1024 * 1024, '先読みせず送信キューを小さく保つ');
  const seqs = await readSeqs(h.client, total);
  assert.deepEqual(seqs, Array.from({ length: total }, (_, i) => i + 1));
  await h.close();
});

test('stream: リングに追い越されたら切断する (読まないクライアントでも検出する)', async () => {
  const h = await harness();
  const payload = 'x'.repeat(256 * 1024);
  const src = source<string>(4 * payload.length); // 4 件ぶんだけ保持
  subscribeToSocket(h, src, undefined);
  h.reply();
  h.client.pause();
  for (let i = 0; i < 60 && !h.conn.socket.destroyed; i++) src.push(payload, payload.length);
  assert.equal(h.conn.socket.destroyed, true);
  await h.close();
  assert.equal(src.listenerCount, 0, '切断で購読が外れる');
});

test('stream: 追い越されていなければ、書けない間に何件溜まっても切らない', async () => {
  const h = await harness();
  const payload = 'x'.repeat(256 * 1024);
  const src = source<string>(1000 * payload.length);
  subscribeToSocket(h, src, undefined);
  h.reply();
  h.client.pause();
  for (let i = 0; i < 100; i++) src.push(payload, payload.length);
  assert.equal(h.conn.socket.destroyed, false);
  const seqs = await readSeqs(h.client, 100);
  assert.equal(seqs.at(-1), 100);
  await h.close();
});

test('stream: since から再生し、head 以前は再生・以降はライブで重複なく届く', async () => {
  const h = await harness();
  const src = source<string>(100);
  for (const c of 'abcde') src.push(c);
  const { seqs, result } = subscribeRecording(h, src, 2);
  assert.deepEqual(result, { gap: false, head: 5, epoch: EPOCH });
  src.push('f'); // 応答前 (afterReply 前) に入った分
  h.reply();
  src.push('g');
  assert.deepEqual(seqs, [3, 4, 5, 6, 7]);
  await h.close();
});

test('stream: since 省略はライブのみ (head 以前は送らない)', async () => {
  const h = await harness();
  const src = source<string>(100);
  for (const c of 'abc') src.push(c);
  const { seqs, result } = subscribeRecording(h, src, undefined);
  assert.equal(result.head, 3);
  h.reply();
  src.push('d');
  assert.deepEqual(seqs, [4]);
  await h.close();
});

test('stream: since が保持範囲より古ければ gap で、保持している最古から再生する', async () => {
  const h = await harness();
  const src = source<string>(3);
  for (const c of 'abcdef') src.push(c); // 保持 seq 4..6
  const { seqs, result } = subscribeRecording(h, src, 1);
  assert.equal(result.gap, true);
  h.reply();
  assert.equal(h.conn.socket.destroyed, false, '再生開始時点の欠落は gap で知らせるだけで切らない');
  assert.deepEqual(seqs, [4, 5, 6]);
  await h.close();
});

test('stream: epoch が違えば gap で、最古から再生する', async () => {
  const h = await harness();
  const src = source<string>(100);
  for (const c of 'abc') src.push(c);
  const { seqs, result } = subscribeRecording(h, src, 3, { epoch: 'old-epoch' });
  assert.equal(result.gap, true);
  h.reply();
  assert.deepEqual(seqs, [1, 2, 3]);
  await h.close();
});

test('stream: since が head より先なら gap で、再生せず次の要素から送る', async () => {
  const h = await harness();
  const src = source<string>(100);
  for (const c of 'abc') src.push(c);
  const { seqs, result } = subscribeRecording(h, src, 1000);
  assert.equal(result.gap, true);
  h.reply();
  src.push('d');
  assert.deepEqual(seqs, [4]);
  await h.close();
});

test('stream: 同じキーの再購読は置き換え、解除後は送らない', async () => {
  const h = await harness();
  const src = source<string>(100);
  const first = subscribeRecording(h, src, undefined);
  h.reply();
  const second = subscribeRecording(h, src, undefined);
  h.reply();
  assert.equal(src.listenerCount, 1, '古い購読は外れている');
  src.push('a');
  assert.deepEqual([first.seqs, second.seqs], [[], [1]]);
  h.conn.subscriptions.get('k')?.();
  src.push('b');
  assert.deepEqual(second.seqs, [1]);
  assert.equal(h.conn.socket.listenerCount('drain'), 0, 'off で drain の購読も外れる');
  await h.close();
});

test('stream: 応答より先に通知しない / 応答前に置き換えられた購読は有効化されない', async () => {
  const h = await harness();
  const src = source<string>(100);
  for (const c of 'ab') src.push(c);
  const replaced = subscribeRecording(h, src, 0);
  const current = subscribeRecording(h, src, 0);
  src.push('c');
  assert.deepEqual([replaced.seqs, current.seqs], [[], []], 'afterReply 前は何も送らない');
  h.reply();
  assert.deepEqual([replaced.seqs, current.seqs], [[], [1, 2, 3]]);
  await h.close();
});
