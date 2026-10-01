import assert from 'node:assert/strict';
import { afterEach, beforeEach, mock, test } from 'node:test';
import { MisaoClient } from '../src/client.js';
import type { ConnectionState } from '../src/client.js';
import type { GapInfo } from '../src/stream-subscriber.js';
import { FakeDaemon, PANE_ID, eventNotification, lineNotification, ulid, waitFor } from './helpers/fake-daemon.js';

const FAST_BACKOFF = { initialDelayMs: 5, maxDelayMs: 20, factor: 2 };
const BOOM = new Error('boom');

let daemon: FakeDaemon;
let client: MisaoClient;
let states: ConnectionState[];
let reported: unknown[];

beforeEach(async () => {
  daemon = new FakeDaemon();
  await daemon.start();
  client = new MisaoClient({ socketPath: daemon.socketPath, backoff: FAST_BACKOFF });
  states = [];
  reported = [];
  client.onStateChange((s) => states.push(s));
  client.onError((e) => reported.push(e));
});

afterEach(async () => {
  mock.restoreAll();
  client.close();
  await daemon.dispose();
});

const connectedCount = (): number => states.filter((s) => s.status === 'connected').length;

test('a throwing gap listener does not stop reconnecting, other streams still get their gap, and the error is reported', async () => {
  await client.connect();
  const seen: number[] = [];
  await client.subscribeEvents((e) => seen.push(e.seq));
  await client.subscribeLines(PANE_ID, () => undefined);
  const gaps: GapInfo[] = [];
  client.onGap(() => {
    throw BOOM;
  });
  client.onGap((g) => gaps.push(g));

  const before = connectedCount();
  await daemon.restart(ulid(2));
  await waitFor(() => connectedCount() === before + 1);

  assert.deepEqual(gaps.map((g) => g.stream.kind).sort(), ['events', 'lines']);
  assert.deepEqual(reported, [BOOM, BOOM]);
  // 復元後も配信が続く
  daemon.notify(eventNotification(1));
  await waitFor(() => seen.length === 1);
});

test('a throwing state listener does not stop the reconnect loop', async () => {
  await client.connect();
  client.onStateChange(() => {
    throw BOOM;
  });
  const before = connectedCount();
  daemon.dropConnections();
  await waitFor(() => connectedCount() === before + 1);
  assert.ok(reported.length >= 2); // reconnecting と connected の両方
  assert.ok(reported.every((e) => e === BOOM));
});

test('a throwing handler does not drop later lines of the same chunk', async () => {
  await client.connect();
  const seen: number[] = [];
  const sub = await client.subscribeEvents((e) => {
    seen.push(e.seq);
    if (e.seq === 1) throw BOOM;
  });
  daemon.notify(eventNotification(1), eventNotification(2), eventNotification(3));
  await waitFor(() => seen.length === 3);
  assert.deepEqual(seen, [1, 2, 3]);
  assert.deepEqual(reported, [BOOM]);
  assert.equal(sub.cursor.seq, 3);
});

test('a throwing line handler is reported and the stream keeps flowing', async () => {
  await client.connect();
  const seen: string[] = [];
  await client.subscribeLines(PANE_ID, (l) => {
    seen.push(l.text);
    throw BOOM;
  });
  daemon.notify(lineNotification(1, 'a'), lineNotification(2, 'b'));
  await waitFor(() => seen.length === 2);
  assert.deepEqual(reported, [BOOM, BOOM]);
});

test('subscribe resolves when a gap listener throws during activation', async () => {
  daemon.handle('events.subscribe', () => ({ result: { gap: true, head: 9, epoch: daemon.epoch } }));
  await client.connect();
  client.onGap(() => {
    throw BOOM;
  });
  const sub = await client.subscribeEvents(() => undefined, { since: 3, epoch: daemon.epoch });
  assert.equal(sub.cursor.seq, 3);
  assert.deepEqual(reported, [BOOM]);
});

test('an unexpected exception inside the reconnect loop is reported and closes the client', async () => {
  const broken = { valueOf: () => { throw BOOM; } } as unknown as number;
  client.close();
  client = new MisaoClient({ socketPath: daemon.socketPath, backoff: { factor: broken } });
  client.onStateChange((s) => states.push(s));
  client.onError((e) => reported.push(e));
  await client.connect();
  daemon.dropConnections();
  await waitFor(() => states.at(-1)?.status === 'closed');
  assert.deepEqual(reported, [BOOM]);
});

test('without an onError listener the error is logged with console.error, not rethrown', async () => {
  const logged = mock.method(console, 'error', (..._args: unknown[]) => undefined);
  const quiet = new MisaoClient({ socketPath: daemon.socketPath, backoff: FAST_BACKOFF });
  try {
    await quiet.connect();
    const seen: number[] = [];
    await quiet.subscribeEvents((e) => {
      seen.push(e.seq);
      throw BOOM;
    });
    daemon.notify(eventNotification(1), eventNotification(2));
    await waitFor(() => seen.length === 2);
    assert.equal(logged.mock.callCount(), 2);
    assert.ok(logged.mock.calls[0]?.arguments.includes(BOOM));
  } finally {
    quiet.close();
  }
});

test('a throwing onError listener falls back to console.error without recursing', async () => {
  const logged = mock.method(console, 'error', (..._args: unknown[]) => undefined);
  const failure = new Error('onError failed');
  client.onError(() => {
    throw failure;
  });
  await client.connect();
  const seen: number[] = [];
  await client.subscribeEvents((e) => {
    seen.push(e.seq);
    throw BOOM;
  });
  daemon.notify(eventNotification(1), eventNotification(2));
  await waitFor(() => seen.length === 2);
  assert.deepEqual(reported, [BOOM, BOOM]); // 先に登録した onError リスナーには届く
  assert.equal(logged.mock.callCount(), 2);
  assert.ok(logged.mock.calls[0]?.arguments.includes(failure));
});
