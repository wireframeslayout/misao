import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import * as net from 'node:net';
import { DEFAULT_MAX_LINE_BYTES } from '@misao/protocol';
import { Connection, MAX_WRITABLE_BYTES, STREAM_HIGH_WATER_BYTES } from '../src/connection.js';

interface Pair {
  conn: Connection;
  client: net.Socket;
  values: unknown[];
  closed: Promise<void>;
  close(): Promise<void>;
}

async function pair(): Promise<Pair> {
  const values: unknown[] = [];
  let resolveClosed!: () => void;
  const closed = new Promise<void>((r) => (resolveClosed = r));
  let conn!: Connection;
  const accepted = new Promise<void>((resolve) => {
    server.once('connection', (socket) => {
      conn = new Connection(socket, {
        onMessage: (_c, v) => values.push(v),
        onClose: () => resolveClosed(),
      });
      resolve();
    });
  });
  const client = net.connect(address());
  await accepted;
  return {
    conn,
    client,
    values,
    closed,
    close: async () => {
      client.destroy();
      await closed;
    },
  };
}

const server = net.createServer();
let sockPath = '';
const address = (): string => sockPath;

before(async () => {
  sockPath = `/tmp/misao-conn-${process.pid}-${Date.now()}.sock`;
  await new Promise<void>((r) => server.listen(sockPath, r));
});

test('connection: 行ごとにパース済みの値を渡し、壊れた JSON は 1 行ごとに Parse error を返す', async () => {
  const p = await pair();
  const got: string[] = [];
  p.client.on('data', (d) => got.push(d.toString()));
  p.client.write('{"a":1}\n{bad\n[]\n');
  await new Promise((r) => setTimeout(r, 100));
  assert.deepEqual(p.values, [{ a: 1 }, []]);
  const replies = got.join('').split('\n').filter(Boolean).map((l) => JSON.parse(l));
  assert.equal(replies.length, 1);
  assert.equal(replies[0].error.code, -32700);
  assert.equal(replies[0].id, null);
  await p.close();
});

test('connection: 1 行が上限を超えたら Parse error を返して切断する', async () => {
  const p = await pair();
  const got: Buffer[] = [];
  p.client.on('data', (d) => got.push(d));
  p.client.on('error', () => undefined);
  const clientClosed = new Promise((r) => p.client.once('close', r));
  p.client.write(Buffer.alloc(DEFAULT_MAX_LINE_BYTES + 1, 0x61));
  // 相手が閉じるのを待たずにサーバー側から閉じる (half-open で残さない)
  await p.closed;
  await clientClosed;
  assert.equal(JSON.parse(Buffer.concat(got).toString().trim()).error.code, -32700);
  assert.deepEqual(p.values, []);
});

test('connection: writableLength が上限を超えたら destroy する', async () => {
  const p = await pair();
  p.client.pause(); // 読まない = 送信キューが溜まる
  const payload = 'x'.repeat(1024 * 1024);
  for (let i = 0; i < MAX_WRITABLE_BYTES / (1024 * 1024) + 4 && !p.conn.socket.destroyed; i++) {
    p.conn.notify('event', i, 'ts', { payload });
  }
  assert.equal(p.conn.socket.destroyed, true);
  await p.closed;
});

test('connection: isStreamWritable は送信キューが STREAM_HIGH_WATER_BYTES 未満の間だけ true で、閉じたら false', async () => {
  const p = await pair();
  p.client.pause();
  assert.equal(p.conn.isStreamWritable(), true);
  const payload = 'x'.repeat(256 * 1024);
  for (let i = 0; i < STREAM_HIGH_WATER_BYTES / payload.length + 2; i++) p.conn.notify('event', i, 'ts', { payload });
  assert.equal(p.conn.isStreamWritable(), false);
  assert.equal(p.conn.socket.destroyed, false, '16 MiB 未満なので切らない');
  await p.close();
  assert.equal(p.conn.isStreamWritable(), false);
});

test('connection: onDrain はキューが空になると呼ばれ、解除後と close 後は呼ばれない', async () => {
  const p = await pair();
  p.client.pause();
  let drained = 0;
  let removed = 0;
  p.conn.onDrain(() => drained++);
  const off = p.conn.onDrain(() => removed++);
  off();
  const payload = 'x'.repeat(256 * 1024);
  for (let i = 0; i < 8; i++) p.conn.notify('event', i, 'ts', { payload });
  assert.equal(drained, 0);
  p.client.resume();
  await waitUntil(() => drained > 0);
  assert.equal(removed, 0);
  assert.equal(p.conn.isStreamWritable(), true);
  await p.close();
  assert.equal(p.conn.socket.listenerCount('drain'), 0);
});

test('connection: onDrain は購読数によらずソケットの drain リスナーを 1 つだけ使い、全員に知らせる', async () => {
  const p = await pair();
  p.client.pause();
  const count = p.conn.socket.getMaxListeners() + 5; // 1 接続で多数のペインを購読する (SDK は接続を共有する)
  const drained = new Array<number>(count).fill(0);
  const offs = drained.map((_, i) => p.conn.onDrain(() => drained[i]!++));
  assert.equal(p.conn.socket.listenerCount('drain'), 1);
  const payload = 'x'.repeat(256 * 1024);
  for (let i = 0; i < 8; i++) p.conn.notify('event', i, 'ts', { payload });
  p.client.resume();
  await waitUntil(() => drained.every((n) => n > 0));
  for (const off of offs) off();
  assert.equal(p.conn.socket.listenerCount('drain'), 0, '全員が解除したらリスナーも外す');
  await p.close();
});

test('connection: close で購読が解除される', async () => {
  const p = await pair();
  const calls: string[] = [];
  p.conn.subscriptions.set('k', () => calls.push('off'));
  await p.close();
  assert.deepEqual(calls, ['off']);
  assert.equal(p.conn.subscriptions.size, 0);
});

async function waitUntil(cond: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !cond(); i++) await new Promise((r) => setTimeout(r, 10));
  assert.ok(cond(), 'condition not met');
}

after(async () => {
  await new Promise<void>((r) => server.close(() => r()));
});
