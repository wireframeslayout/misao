import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import * as net from 'node:net';
import { DEFAULT_MAX_LINE_BYTES } from '@misao/protocol';
import { Connection, MAX_WRITABLE_BYTES } from '../src/connection.js';

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

test('connection: close で購読が解除される', async () => {
  const p = await pair();
  const calls: string[] = [];
  p.conn.subscriptions.set('k', () => calls.push('off'));
  await p.close();
  assert.deepEqual(calls, ['off']);
  assert.equal(p.conn.subscriptions.size, 0);
});

after(async () => {
  await new Promise<void>((r) => server.close(() => r()));
});
