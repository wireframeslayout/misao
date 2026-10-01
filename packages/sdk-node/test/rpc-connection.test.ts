import assert from 'node:assert/strict';
import { afterEach, beforeEach, test } from 'node:test';
import { ErrorCode } from '@misao/protocol';
import { MisaoConnectionError, MisaoRpcError } from '../src/errors.js';
import { RpcConnection } from '../src/rpc-connection.js';
import type { Notification } from '../src/rpc-connection.js';
import { FakeDaemon, PANE_ID, eventNotification, lineNotification, notification, waitFor } from './helpers/fake-daemon.js';

let daemon: FakeDaemon;
let conn: RpcConnection;

beforeEach(async () => {
  daemon = new FakeDaemon();
  await daemon.start();
  conn = await RpcConnection.connect(daemon.socketPath);
});

afterEach(async () => {
  conn.close();
  await daemon.dispose();
});

function collectClose(): { reasons: MisaoConnectionError[] } {
  const reasons: MisaoConnectionError[] = [];
  conn.onClose((reason) => reasons.push(reason));
  return { reasons };
}

test('request returns the validated result', async () => {
  const info = await conn.request('server.info', {});
  assert.equal(info.epoch, daemon.epoch);
});

test('connect rejects with MisaoConnectionError when nobody listens', async () => {
  await assert.rejects(RpcConnection.connect(`${daemon.dir}/missing.sock`), MisaoConnectionError);
});

test('invalid params are rejected before anything is sent', async () => {
  await assert.rejects(conn.request('events.subscribe', { since: -1 }));
  assert.equal(daemon.received('events.subscribe').length, 0);
});

test('an error response becomes MisaoRpcError', async () => {
  daemon.handle('pane.subscribe_lines', () => ({ error: { code: ErrorCode.PaneNotFound, message: 'no pane' } }));
  await assert.rejects(conn.request('pane.subscribe_lines', { paneId: PANE_ID }), (error: unknown) => {
    assert.ok(error instanceof MisaoRpcError);
    assert.equal(error.code, ErrorCode.PaneNotFound);
    return true;
  });
});

test('an invalid result is a protocol violation that closes the connection', async () => {
  daemon.handle('server.info', () => ({ result: { nope: true } }));
  const { reasons } = collectClose();
  await assert.rejects(conn.request('server.info', {}), MisaoConnectionError);
  await waitFor(() => reasons.length === 1);
  assert.match(reasons[0]?.message ?? '', /invalid result/);
});

test('pending requests reject when the connection drops', async () => {
  daemon.handle('server.info', () => {
    daemon.dropConnections();
    return {};
  });
  await assert.rejects(conn.request('server.info', {}), MisaoConnectionError);
  await assert.rejects(conn.request('server.info', {}), /not connected/);
});

test('invalid JSON closes the connection as a protocol violation', async () => {
  const { reasons } = collectClose();
  daemon.writeRaw('not json\n');
  await waitFor(() => reasons.length === 1);
  assert.match(reasons[0]?.message ?? '', /invalid JSON/);
  assert.equal(conn.isClosed, true);
});

test('a line over the limit closes the connection', async () => {
  const { reasons } = collectClose();
  daemon.writeRaw('x'.repeat(9 * 1024 * 1024));
  await waitFor(() => reasons.length === 1);
  assert.match(reasons[0]?.message ?? '', /exceeds/);
});

test('a response with an unknown id closes the connection', async () => {
  const { reasons } = collectClose();
  daemon.writeRaw('{"jsonrpc":"2.0","id":999,"result":{}}\n');
  await waitFor(() => reasons.length === 1);
  assert.match(reasons[0]?.message ?? '', /unknown id/);
});

test('notifications are validated and delivered', async () => {
  const received: Notification[] = [];
  conn.onNotification((n) => received.push(n));
  daemon.notify(eventNotification(1), lineNotification(2, 'hi'));
  await waitFor(() => received.length === 2);
  assert.equal(received[0]?.method, 'event');
  assert.equal(received[1]?.method, 'pane.line');
});

test('a known notification with invalid params closes the connection', async () => {
  const { reasons } = collectClose();
  daemon.notify(notification('pane.line', { seq: 1 }));
  await waitFor(() => reasons.length === 1);
  assert.match(reasons[0]?.message ?? '', /invalid pane.line/);
});

test('unknown notification names are ignored', async () => {
  const received: Notification[] = [];
  conn.onNotification((n) => received.push(n));
  daemon.notify(notification('future.thing', {}), eventNotification(1));
  await waitFor(() => received.length === 1);
  assert.equal(conn.isClosed, false);
});

test('requestSync settles before notifications that follow in the same chunk', async () => {
  daemon.handle('events.subscribe', () => ({
    result: { gap: false, head: 0, epoch: daemon.epoch },
    trailing: [eventNotification(1)],
  }));
  const order: string[] = [];
  conn.onNotification(() => order.push('notification'));
  conn.requestSync('events.subscribe', {}, () => order.push('settled'));
  await waitFor(() => order.length === 2);
  assert.deepEqual(order, ['settled', 'notification']);
});

test('onClose and onNotification return an unsubscribe function', async () => {
  const received: Notification[] = [];
  const off = conn.onNotification((n) => received.push(n));
  off();
  daemon.notify(eventNotification(1));
  await conn.request('server.info', {});
  assert.equal(received.length, 0);
});
