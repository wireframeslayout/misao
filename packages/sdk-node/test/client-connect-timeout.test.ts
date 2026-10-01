import assert from 'node:assert/strict';
import { afterEach, beforeEach, test } from 'node:test';
import { PROTOCOL_VERSION } from '@misao/protocol';
import { MisaoClient } from '../src/client.js';
import type { ConnectionState } from '../src/client.js';
import { MisaoConnectionError } from '../src/errors.js';
import { FakeDaemon, waitFor } from './helpers/fake-daemon.js';

const FAST_BACKOFF = { initialDelayMs: 5, maxDelayMs: 20, factor: 2 };
const TIMEOUT_MS = 40;

let daemon: FakeDaemon;
let client: MisaoClient;
let states: ConnectionState[];

beforeEach(async () => {
  daemon = new FakeDaemon();
  await daemon.start();
  client = new MisaoClient({ socketPath: daemon.socketPath, backoff: FAST_BACKOFF, connectTimeoutMs: TIMEOUT_MS });
  states = [];
  client.onStateChange((s) => states.push(s));
});

afterEach(async () => {
  client.close();
  await daemon.dispose();
});

test('the initial connect rejects when the daemon never answers server.info, and closes the socket', async () => {
  daemon.handle('server.info', () => 'hold');
  await assert.rejects(client.connect(), (error: unknown) => {
    assert.ok(error instanceof MisaoConnectionError);
    assert.match(error.message, /timed out after 40ms/);
    return true;
  });
  await waitFor(() => daemon.connectionCount === 0);
  assert.deepEqual(states, []);
  // idle に戻るので、デーモンが回復すれば再試行できる
  daemon.handle('server.info', () => ({
    result: { protocolVersion: PROTOCOL_VERSION, pid: process.pid, epoch: daemon.epoch, uptimeSec: 0, paneCount: 0, eventHead: 0 },
  }));
  await client.connect();
});

test('a hung reconnect attempt times out and is retried', async () => {
  await client.connect();
  const info = {
    result: { protocolVersion: PROTOCOL_VERSION, pid: process.pid, epoch: daemon.epoch, uptimeSec: 0, paneCount: 0, eventHead: 0 },
  };
  let attempts = 0;
  daemon.handle('server.info', () => (++attempts <= 2 ? 'hold' : info));
  daemon.dropConnections();
  await waitFor(() => states.filter((s) => s.status === 'connected').length === 2);
  assert.equal(attempts, 3);
  const reconnecting = states.flatMap((s) => (s.status === 'reconnecting' ? [s] : []));
  assert.ok(reconnecting.length >= 3);
  assert.match(reconnecting[1]?.cause.message ?? '', /timed out/);
});

test('the timer is cleared once the connection is established', async () => {
  await client.connect();
  await new Promise((resolve) => setTimeout(resolve, TIMEOUT_MS * 2));
  assert.deepEqual(states, [{ status: 'connected' }]);
  assert.equal(daemon.connectionCount, 1);
});
