import assert from 'node:assert/strict';
import * as net from 'node:net';
import { afterEach, beforeEach, test } from 'node:test';
import { duplexPair } from 'node:stream';
import type { Duplex } from 'node:stream';
import { LineSplitter, PROTOCOL_VERSION, encodeMessage } from '@misao/protocol';
import { MisaoClient } from '../src/client.js';
import type { ConnectionState } from '../src/client.js';
import { MisaoConnectionError } from '../src/errors.js';
import type { GapInfo } from '../src/stream-subscriber.js';
import { FakeDaemon, PANE_ID, eventNotification, lineNotification, ulid, waitFor } from './helpers/fake-daemon.js';

const FAST_BACKOFF = { initialDelayMs: 5, maxDelayMs: 20, factor: 2 };

let daemon: FakeDaemon;
let client: MisaoClient | undefined;
let states: ConnectionState[];
let connectCalls: number;

function newClient(): MisaoClient {
  const created = new MisaoClient({
    connect: async () => {
      connectCalls++;
      return net.connect(daemon.socketPath);
    },
    backoff: FAST_BACKOFF,
  });
  created.onStateChange((s) => states.push(s));
  client = created;
  return created;
}

beforeEach(async () => {
  daemon = new FakeDaemon();
  await daemon.start();
  states = [];
  connectCalls = 0;
  client = undefined;
});

afterEach(async () => {
  client?.close();
  await daemon.dispose();
});

const connectedCount = (): number => states.filter((s) => s.status === 'connected').length;

test('connect option: request works and connect() is used instead of socketPath', async () => {
  const c = newClient();
  await c.connect();
  const info = await c.request('server.info', {});
  assert.equal(info.epoch, daemon.epoch);
  assert.equal(connectCalls, 1);
});

test('connect option: reconnects with backoff after a drop and calls connect() again', async () => {
  const c = newClient();
  await c.connect();
  daemon.dropConnections();
  await waitFor(() => connectedCount() === 2);
  assert.equal(connectCalls, 2);
  assert.ok(states.some((s) => s.status === 'reconnecting'));
  assert.equal((await c.request('server.info', {})).epoch, daemon.epoch);
});

test('connect option: a rejected first connect() leaves the client idle and a second connect() succeeds', async () => {
  let failures = 1;
  const c = new MisaoClient({
    connect: async () => {
      if (failures > 0) {
        failures--;
        throw new Error('relay down');
      }
      return net.connect(daemon.socketPath);
    },
    backoff: FAST_BACKOFF,
  });
  client = c;
  c.onStateChange((s) => states.push(s));
  await c.connect().then(
    () => assert.fail('first connect should reject'),
    (error: unknown) => {
      assert.ok(error instanceof MisaoConnectionError);
      assert.match(error.message, /relay down/);
    },
  );
  await c.connect();
  daemon.dropConnections();
  await waitFor(() => connectedCount() === 2);
});

test('connect option: a failing connect() during reconnect is retried with reconnecting states', async () => {
  let down = false;
  const c = new MisaoClient({
    connect: async () => {
      if (down) throw new Error('relay down');
      return net.connect(daemon.socketPath);
    },
    backoff: FAST_BACKOFF,
  });
  client = c;
  c.onStateChange((s) => states.push(s));
  await c.connect();
  down = true;
  daemon.dropConnections();
  await waitFor(() => states.filter((s) => s.status === 'reconnecting').length >= 3);
  down = false;
  await waitFor(() => connectedCount() === 2);
});

test('connect option: subscriptions are restored with since and epoch after reconnect', async () => {
  const c = newClient();
  await c.connect();
  const lines: number[] = [];
  const events: number[] = [];
  await c.subscribeLines(PANE_ID, (l) => lines.push(l.seq));
  await c.subscribeEvents((e) => events.push(e.seq));
  daemon.notify(lineNotification(1, 'a'), eventNotification(1), lineNotification(2, 'b'));
  await waitFor(() => lines.length === 2 && events.length === 1);

  daemon.dropConnections();
  await waitFor(() => connectedCount() === 2);
  const resubscribe = daemon.received('pane.subscribe_lines').at(-1);
  assert.equal(resubscribe?.params['since'], 2);
  assert.equal(resubscribe?.params['epoch'], daemon.epoch);
  assert.equal(daemon.received('events.subscribe').at(-1)?.params['since'], 1);

  daemon.notify(lineNotification(2, 'dup'), lineNotification(3, 'c'), eventNotification(2));
  await waitFor(() => lines.length === 3 && events.length === 2);
  assert.deepEqual(lines, [1, 2, 3]);
});

test('connect option: onGap fires with reason epoch when the daemon restarted', async () => {
  const c = newClient();
  const gaps: GapInfo[] = [];
  c.onGap((g) => gaps.push(g));
  await c.connect();
  await c.subscribeEvents(() => undefined);
  daemon.notify(eventNotification(5));
  await daemon.restart(ulid(2));
  await waitFor(() => connectedCount() === 2);
  await waitFor(() => gaps.length === 1);
  assert.equal(gaps[0]?.reason, 'epoch');
  assert.deepEqual(gaps[0]?.stream, { kind: 'events' });
});

/** デーモン役を PassThrough ペア (duplexPair) の片側で動かす。 */
function serveOnPair(onRequest: (id: number, method: string) => unknown): { client: Duplex; server: Duplex } {
  const [clientSide, serverSide] = duplexPair();
  const splitter = new LineSplitter();
  serverSide.on('data', (chunk: Buffer) => {
    for (const line of splitter.push(chunk)) {
      const { id, method } = JSON.parse(line) as { id: number; method: string };
      serverSide.write(encodeMessage({ jsonrpc: '2.0', id, result: onRequest(id, method) }));
    }
  });
  return { client: clientSide, server: serverSide };
}

test('connect option: works over an in-process duplex pair, including drop and reconnect', async () => {
  const serverInfo = {
    protocolVersion: PROTOCOL_VERSION,
    pid: 1,
    epoch: ulid(1),
    uptimeSec: 0,
    paneCount: 0,
    eventHead: 0,
  };
  const servers: Duplex[] = [];
  const c = new MisaoClient({
    connect: async () => {
      const pair = serveOnPair(() => serverInfo);
      servers.push(pair.server);
      return pair.client;
    },
    backoff: FAST_BACKOFF,
  });
  client = c;
  c.onStateChange((s) => states.push(s));
  await c.connect();
  assert.equal((await c.request('server.info', {})).epoch, serverInfo.epoch);

  servers[0]?.end(); // 相手の end だけでも切断として扱う
  await waitFor(() => connectedCount() === 2);
  assert.equal(servers.length, 2);
  assert.equal((await c.request('server.info', {})).epoch, serverInfo.epoch);
});

test('connect option: a non-Error rejection reason still yields a readable MisaoConnectionError', async () => {
  for (const reason of [undefined, null, 'plain string']) {
    const c = new MisaoClient({ connect: () => Promise.reject(reason) });
    await assert.rejects(c.connect(), (error: unknown) => {
      assert.ok(error instanceof MisaoConnectionError);
      assert.match(error.message, new RegExp(`cannot connect: ${String(reason)}`));
      return true;
    });
  }
});

test('connect option: an already destroyed Duplex fails the attempt', async () => {
  const c = new MisaoClient({
    connect: async () => {
      const [side] = duplexPair();
      side.destroy();
      return side;
    },
  });
  await assert.rejects(c.connect(), /already destroyed/);
});

/** 解決を外から制御できる connect。signal と、解決した Duplex を記録する。 */
function deferredConnect(): {
  connect: (options: { signal: AbortSignal }) => Promise<Duplex>;
  signals: AbortSignal[];
  resolveLast: (stream: Duplex) => void;
} {
  const signals: AbortSignal[] = [];
  const resolvers: Array<(stream: Duplex) => void> = [];
  return {
    signals,
    resolveLast: (stream) => resolvers.at(-1)?.(stream),
    connect: ({ signal }) => {
      signals.push(signal);
      return new Promise<Duplex>((resolve) => resolvers.push(resolve));
    },
  };
}

test('connect option: a hanging connect() times out, aborts its signal and the late Duplex is destroyed', async () => {
  const d = deferredConnect();
  const c = new MisaoClient({ connect: d.connect, connectTimeoutMs: 30 });
  client = c;
  await assert.rejects(c.connect(), /timed out after 30ms/);
  assert.equal(d.signals[0]?.aborted, true);
  const [late] = duplexPair();
  d.resolveLast(late);
  await waitFor(() => late.destroyed);
});

test('connect option: the reconnect loop continues after a connect() timeout', async () => {
  let hang = false;
  const signals: AbortSignal[] = [];
  const c = new MisaoClient({
    connect: ({ signal }) => {
      signals.push(signal);
      return hang ? new Promise<Duplex>(() => undefined) : Promise.resolve(net.connect(daemon.socketPath));
    },
    backoff: FAST_BACKOFF,
    connectTimeoutMs: 30,
  });
  client = c;
  c.onStateChange((s) => states.push(s));
  await c.connect();
  hang = true;
  daemon.dropConnections();
  await waitFor(() => states.filter((s) => s.status === 'reconnecting').length >= 2); // 1 回目が timeout して再試行
  assert.ok(signals.slice(1).some((s) => s.aborted));
  hang = false;
  await waitFor(() => connectedCount() === 2);
});

test('connect option: close() during a pending first connect() aborts the signal and destroys the late Duplex', async () => {
  const d = deferredConnect();
  const c = new MisaoClient({ connect: d.connect });
  client = c;
  const pending = c.connect();
  await waitFor(() => d.signals.length === 1);
  c.close();
  await assert.rejects(pending, MisaoConnectionError);
  assert.equal(d.signals[0]?.aborted, true);
  const [late] = duplexPair();
  d.resolveLast(late);
  await waitFor(() => late.destroyed);
});

test('connect option: close() during a pending reconnect connect() aborts it and stops reconnecting', async () => {
  const signals: AbortSignal[] = [];
  let hang = false;
  const c = new MisaoClient({
    connect: ({ signal }) => {
      signals.push(signal);
      return hang ? new Promise<Duplex>(() => undefined) : Promise.resolve(net.connect(daemon.socketPath));
    },
    backoff: FAST_BACKOFF,
  });
  client = c;
  c.onStateChange((s) => states.push(s));
  await c.connect();
  hang = true;
  daemon.dropConnections();
  await waitFor(() => signals.length === 2);
  c.close();
  assert.equal(signals[1]?.aborted, true);
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.equal(signals.length, 2);
  assert.equal(states.at(-1)?.status, 'closed');
});

test('constructor rejects when neither or both of socketPath and connect are given', () => {
  const connect = async (): Promise<Duplex> => net.connect(daemon.socketPath);
  assert.throws(() => new MisaoClient({} as never), TypeError);
  assert.throws(() => new MisaoClient({ connect: 'x' } as never), TypeError);
  assert.throws(() => new MisaoClient({ socketPath: 5 } as never), TypeError);
  assert.throws(() => new MisaoClient({ socketPath: daemon.socketPath, connect } as never), TypeError);
});

test('socketPath option still works unchanged', async () => {
  const c = new MisaoClient({ socketPath: daemon.socketPath, backoff: FAST_BACKOFF });
  client = c;
  await c.connect();
  assert.equal((await c.request('server.info', {})).epoch, daemon.epoch);
});
