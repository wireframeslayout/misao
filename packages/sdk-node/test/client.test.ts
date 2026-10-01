import assert from 'node:assert/strict';
import { afterEach, beforeEach, test } from 'node:test';
import { ErrorCode } from '@misao/protocol';
import type { EventParams } from '@misao/protocol';
import { MisaoClient } from '../src/client.js';
import type { ConnectionState } from '../src/client.js';
import { MisaoConnectionError, MisaoProtocolVersionError } from '../src/errors.js';
import type { GapInfo, SubscriptionErrorInfo } from '../src/stream-subscriber.js';
import {
  FakeDaemon,
  PANE_ID,
  eventNotification,
  lineNotification,
  notification,
  ulid,
  waitFor,
} from './helpers/fake-daemon.js';

const FAST_BACKOFF = { initialDelayMs: 5, maxDelayMs: 20, factor: 2 };

let daemon: FakeDaemon;
let client: MisaoClient;
let states: ConnectionState[];
let gaps: GapInfo[];
let subscriptionErrors: SubscriptionErrorInfo[];

beforeEach(async () => {
  daemon = new FakeDaemon();
  await daemon.start();
  client = new MisaoClient({ socketPath: daemon.socketPath, backoff: FAST_BACKOFF });
  states = [];
  gaps = [];
  subscriptionErrors = [];
  client.onStateChange((s) => states.push(s));
  client.onGap((g) => gaps.push(g));
  client.onSubscriptionError((e) => subscriptionErrors.push(e));
});


afterEach(async () => {
  client.close();
  await daemon.dispose();
});

const connectedCount = (): number => states.filter((s) => s.status === 'connected').length;

async function dropAndWaitReconnect(): Promise<void> {
  const before = connectedCount();
  daemon.dropConnections();
  await waitFor(() => connectedCount() === before + 1);
}

test('connect then a typed request', async () => {
  await client.connect();
  const info = await client.request('server.info', {});
  assert.equal(info.epoch, daemon.epoch);
  assert.deepEqual(states, [{ status: 'connected' }]);
});

test('requests before connect reject with MisaoConnectionError', async () => {
  await assert.rejects(client.request('server.info', {}), MisaoConnectionError);
});

test('the initial connect rejects when the daemon is absent, and can be retried', async () => {
  await daemon.stop();
  await assert.rejects(client.connect(), MisaoConnectionError);
  await daemon.start();
  await client.connect();
  assert.equal(connectedCount(), 1);
});

test('an incompatible protocol major rejects connect', async () => {
  daemon.protocolVersion = '99.0.0';
  await assert.rejects(client.connect(), MisaoProtocolVersionError);
});

test('events are delivered in order and duplicates are dropped', async () => {
  await client.connect();
  const seen: number[] = [];
  const sub = await client.subscribeEvents((e) => seen.push(e.seq));
  daemon.notify(eventNotification(1), eventNotification(2), eventNotification(2), eventNotification(3));
  await waitFor(() => seen.length === 3);
  assert.deepEqual(seen, [1, 2, 3]);
  assert.deepEqual(sub.cursor, { seq: 3, epoch: daemon.epoch });
});

test('a live-only subscription resumes from the head reported at subscribe time', async () => {
  daemon.handle('events.subscribe', () => ({ result: { gap: false, head: 10, epoch: daemon.epoch } }));
  await client.connect();
  await client.subscribeEvents(() => undefined);
  await dropAndWaitReconnect();
  assert.deepEqual(daemon.received('events.subscribe').at(-1)?.params, { since: 10, epoch: daemon.epoch });
});

test('subscribing to the same stream twice is an error', async () => {
  await client.connect();
  await client.subscribeEvents(() => undefined);
  await assert.rejects(client.subscribeEvents(() => undefined), /already registered/);
  await client.subscribeLines(PANE_ID, () => undefined);
  await assert.rejects(client.subscribeLines(PANE_ID, () => undefined), /already registered/);
});

test('reconnect resubscribes every stream with since=lastSeq and epoch', async () => {
  await client.connect();
  const events: number[] = [];
  const lines: string[] = [];
  await client.subscribeEvents((e) => events.push(e.seq));
  await client.subscribeLines(PANE_ID, (l) => lines.push(l.text));
  daemon.notify(eventNotification(1), eventNotification(2), lineNotification(1, 'a'));
  await waitFor(() => events.length === 2 && lines.length === 1);

  await dropAndWaitReconnect();

  assert.deepEqual(daemon.received('events.subscribe').at(-1)?.params, { since: 2, epoch: daemon.epoch });
  assert.deepEqual(daemon.received('pane.subscribe_lines').at(-1)?.params, {
    paneId: PANE_ID,
    since: 1,
    epoch: daemon.epoch,
  });
  assert.equal(gaps.length, 0);
  const reconnecting = states.find((s) => s.status === 'reconnecting');
  assert.ok(reconnecting && reconnecting.status === 'reconnecting');
  assert.equal(reconnecting.attempt, 1);
  assert.equal(reconnecting.delayMs, FAST_BACKOFF.initialDelayMs);
  assert.ok(reconnecting.cause instanceof MisaoConnectionError);
});

test('the backoff grows while the daemon stays down, then attempt starts over', async () => {
  await client.connect();
  await daemon.stop();
  await waitFor(() => states.filter((s) => s.status === 'reconnecting').length >= 4);
  const delays = states.flatMap((s) => (s.status === 'reconnecting' ? [s.delayMs] : []));
  assert.deepEqual(delays.slice(0, 4), [5, 10, 20, 20]);

  await daemon.start();
  await waitFor(() => connectedCount() === 2);
  await dropAndWaitReconnect();
  const last = states.filter((s) => s.status === 'reconnecting').at(-1);
  assert.ok(last && last.status === 'reconnecting');
  assert.equal(last.attempt, 1);
});

test('an epoch change discards since, resubscribes from 0 and reports gap(epoch)', async () => {
  await client.connect();
  const seen: number[] = [];
  await client.subscribeEvents((e) => seen.push(e.seq));
  daemon.notify(eventNotification(5));
  await waitFor(() => seen.length === 1);

  const before = connectedCount();
  await daemon.restart(ulid(2));
  await waitFor(() => connectedCount() === before + 1);

  assert.deepEqual(daemon.received('events.subscribe').at(-1)?.params, { since: 0, epoch: ulid(2) });
  assert.deepEqual(gaps, [{ stream: { kind: 'events' }, reason: 'epoch' }]);
  // 巻き戻った seq (1) も届く
  daemon.notify(eventNotification(1));
  await waitFor(() => seen.length === 2);
  assert.deepEqual(seen, [5, 1]);
});

test('an epoch reset with a truncated replay reports only one gap per stream', async () => {
  await client.connect();
  await client.subscribeEvents(() => undefined);
  daemon.handle('events.subscribe', () => ({ result: { gap: true, head: 50, epoch: daemon.epoch } }));
  const before = connectedCount();
  await daemon.restart(ulid(2));
  await waitFor(() => connectedCount() === before + 1);
  assert.deepEqual(gaps.map((g) => g.reason), ['epoch']);
});

test('gap:true with the same epoch is reported as truncated', async () => {
  daemon.handle('events.subscribe', () => ({ result: { gap: true, head: 100, epoch: daemon.epoch } }));
  await client.connect();
  const sub = await client.subscribeEvents(() => undefined, { since: 3, epoch: daemon.epoch });
  assert.deepEqual(gaps, [{ stream: { kind: 'events' }, reason: 'truncated' }]);
  assert.equal(sub.cursor.seq, 3);
});

test('a supplied epoch that differs from the daemon is reported as epoch and since is dropped', async () => {
  daemon.handle('events.subscribe', () => ({ result: { gap: true, head: 4, epoch: daemon.epoch } }));
  await client.connect();
  const seen: number[] = [];
  const sub = await client.subscribeEvents((e) => seen.push(e.seq), { since: 90, epoch: ulid(9) });
  assert.deepEqual(gaps.map((g) => g.reason), ['epoch']);
  assert.equal(sub.cursor.seq, 0);
  daemon.notify(eventNotification(1));
  await waitFor(() => seen.length === 1);
});

test('the subscribe result is applied before replay that arrives in the same chunk', async () => {
  const log: string[] = [];
  daemon.handle('events.subscribe', () => ({
    result: { gap: true, head: 2, epoch: daemon.epoch },
    trailing: [eventNotification(1), eventNotification(2)],
  }));
  await client.connect();
  client.onGap(() => log.push('gap'));
  await client.subscribeEvents((e: EventParams) => log.push(`event:${e.seq}`), { since: 0, epoch: daemon.epoch });
  await waitFor(() => log.length === 3);
  assert.deepEqual(log, ['gap', 'event:1', 'event:2']);
});

test('a resubscribe response is applied before replay in the same chunk (gap precedes data)', async () => {
  await client.connect();
  const log: string[] = [];
  client.onGap((g) => log.push(`gap:${g.reason}`));
  await client.subscribeEvents((e) => log.push(`event:${e.seq}`));
  daemon.notify(eventNotification(1));
  await waitFor(() => log.length === 1);
  log.length = 0;
  daemon.handle('events.subscribe', () => ({
    result: { gap: true, head: 3, epoch: daemon.epoch },
    trailing: [eventNotification(2), eventNotification(3)],
  }));
  await dropAndWaitReconnect();
  await waitFor(() => log.length === 3);
  assert.deepEqual(log, ['gap:truncated', 'event:2', 'event:3']);
});

test('PaneNotFound on resubscribe removes the stream and reports it', async () => {
  await client.connect();
  const lines: string[] = [];
  await client.subscribeLines(PANE_ID, (l) => lines.push(l.text));
  daemon.handle('pane.subscribe_lines', () => ({ error: { code: ErrorCode.PaneNotFound, message: 'gone' } }));
  await dropAndWaitReconnect();

  assert.equal(subscriptionErrors.length, 1);
  assert.deepEqual(subscriptionErrors[0]?.stream, { kind: 'lines', paneId: PANE_ID });
  assert.equal(subscriptionErrors[0]?.error.code, ErrorCode.PaneNotFound);

  const resubscribes = daemon.received('pane.subscribe_lines').length;
  await dropAndWaitReconnect();
  assert.equal(daemon.received('pane.subscribe_lines').length, resubscribes);
  // 外れたストリームの通知は届かない
  daemon.notify(lineNotification(1, 'late'));
  await client.request('server.info', {});
  assert.deepEqual(lines, []);
});

test('unsubscribe stops delivery and the resubscription', async () => {
  await client.connect();
  const seen: number[] = [];
  const sub = await client.subscribeEvents((e) => seen.push(e.seq));
  sub.unsubscribe();
  daemon.notify(eventNotification(1));
  await client.request('server.info', {});
  assert.deepEqual(seen, []);
  const count = daemon.received('events.subscribe').length;
  await dropAndWaitReconnect();
  assert.equal(daemon.received('events.subscribe').length, count);
  // 解除後は同じ stream を登録し直せる
  await client.subscribeEvents(() => undefined);
});

test('pane.output is forwarded to onNotification', async () => {
  await client.connect();
  const outputs: string[] = [];
  client.onNotification((n) => {
    if (n.method === 'pane.output') outputs.push(n.params.dataB64);
  });
  daemon.notify(
    notification('pane.output', { seq: 1, ts: '2026-01-01T00:00:00.000Z', paneId: PANE_ID, dataB64: 'aGk=' }),
  );
  await waitFor(() => outputs.length === 1);
  assert.deepEqual(outputs, ['aGk=']);
});

test('requests reject while reconnecting', async () => {
  await client.connect();
  await daemon.stop();
  await waitFor(() => states.some((s) => s.status === 'reconnecting'));
  await assert.rejects(client.request('server.info', {}), MisaoConnectionError);
});

test('close stops reconnecting and is idempotent', async () => {
  await client.connect();
  await daemon.stop();
  await waitFor(() => states.some((s) => s.status === 'reconnecting'));
  client.close();
  client.close();
  const count = states.length;
  await daemon.start();
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.equal(states.length, count);
  assert.deepEqual(states.at(-1), { status: 'closed' });
  assert.equal(daemon.connectionCount, 0);
});

test('close while connected does not reconnect', async () => {
  await client.connect();
  client.close();
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.equal(states.filter((s) => s.status === 'reconnecting').length, 0);
  await assert.rejects(client.connect(), /closed/);
});

test('an incompatible daemon on reconnect stops reconnecting and closes with the cause', async () => {
  await client.connect();
  daemon.protocolVersion = '99.0.0';
  daemon.dropConnections();
  await waitFor(() => states.at(-1)?.status === 'closed');
  const closed = states.at(-1);
  assert.ok(closed?.status === 'closed' && closed.cause instanceof MisaoProtocolVersionError);
  const attempts = daemon.received('server.info').length;
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.equal(daemon.received('server.info').length, attempts);
  await assert.rejects(client.request('server.info', {}), MisaoConnectionError);
});

test('close while the setup is pending closes the socket and rejects connect', async () => {
  daemon.handle('server.info', () => 'hold');
  const connecting = client.connect();
  await waitFor(() => daemon.isHolding('server.info'));
  client.close();
  await assert.rejects(connecting, MisaoConnectionError);
  await waitFor(() => daemon.connectionCount === 0);
  assert.deepEqual(states, [{ status: 'closed' }]);
});

test('a gap listener can issue requests while streams are being restored', async () => {
  await client.connect();
  await client.subscribeEvents(() => undefined);
  const recovered: Promise<unknown>[] = [];
  client.onGap(() => recovered.push(client.request('server.info', {})));
  const before = connectedCount();
  await daemon.restart(ulid(2));
  await waitFor(() => connectedCount() === before + 1);
  assert.equal(recovered.length, 1);
  const info = (await recovered[0]) as { epoch: string };
  assert.equal(info.epoch, ulid(2));
});

test('a live-only subscription with an epoch but no since starts at head', async () => {
  daemon.handle('events.subscribe', () => ({ result: { gap: false, head: 7, epoch: daemon.epoch } }));
  await client.connect();
  const sub = await client.subscribeEvents(() => undefined, { epoch: ulid(9) });
  assert.equal(sub.cursor.seq, 7);
  assert.deepEqual(gaps, []);
  await dropAndWaitReconnect();
  assert.deepEqual(daemon.received('events.subscribe').at(-1)?.params, { since: 7, epoch: daemon.epoch });
});

test('a stream unsubscribed while its resubscription is pending is not reported as an error', async () => {
  await client.connect();
  const sub = await client.subscribeLines(PANE_ID, () => undefined);
  daemon.handle('pane.subscribe_lines', () => 'hold');
  daemon.dropConnections();
  await waitFor(() => daemon.isHolding('pane.subscribe_lines'));
  sub.unsubscribe();
  const before = connectedCount();
  daemon.release('pane.subscribe_lines', { error: { code: ErrorCode.PaneNotFound, message: 'gone' } });
  await waitFor(() => connectedCount() === before + 1);
  assert.deepEqual(subscriptionErrors, []);
});
