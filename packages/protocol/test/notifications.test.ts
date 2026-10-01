import assert from 'node:assert/strict';
import { test } from 'node:test';
import { knownEventData, parseKnownEvent } from '../src/events.js';
import { EventParamsSchema, PaneLineParamsSchema, PaneOutputParamsSchema } from '../src/notifications.js';
import { PANE_ID, TS } from './fixtures.js';

test('pane.output / pane.line / event の通知', () => {
  const out = PaneOutputParamsSchema.parse({ seq: 1, ts: TS, paneId: PANE_ID, dataB64: 'AA==', replay: 'snapshot' });
  assert.equal(out.replay, 'snapshot');
  assert.equal(PaneOutputParamsSchema.parse({ seq: 1, ts: TS, paneId: PANE_ID, dataB64: 'AA==' }).replay, undefined);
  assert.equal(PaneLineParamsSchema.safeParse({ seq: 1, ts: TS, paneId: PANE_ID, text: 'hi' }).success, true);
  assert.equal(PaneLineParamsSchema.safeParse({ seq: 0.5, ts: TS, paneId: PANE_ID, text: 'hi' }).success, false);
  assert.equal(EventParamsSchema.safeParse({ seq: 1, ts: TS, type: 'pane.closed', paneId: PANE_ID, data: {} }).success, true);
});

test('未知の type は外側のスキーマを通り、parseKnownEvent は undefined', () => {
  const ev = EventParamsSchema.parse({ seq: 1, ts: TS, type: 'future.thing', data: { x: 1 } });
  assert.equal(parseKnownEvent(ev), undefined);
});

test('既知の type は data を検証して型付きで返す', () => {
  const ev = EventParamsSchema.parse({ seq: 2, ts: TS, type: 'pane.exited', paneId: PANE_ID, data: { exitCode: 0, signal: null } });
  const parsed = parseKnownEvent(ev);
  assert.ok(parsed);
  // 型テスト: seq / ts / paneId は名前付きの型を保つ
  const seq: number = parsed.seq;
  const ts: string = parsed.ts;
  const paneId: string | undefined = parsed.paneId;
  // @ts-expect-error seq は string ではない
  const notString: string = parsed.seq;
  // @ts-expect-error paneId は undefined になりうる
  const notOptional: string = parsed.paneId;
  assert.deepEqual([seq, ts, paneId, notString, notOptional], [2, TS, PANE_ID, 2, PANE_ID]);
  // 型の確認は assert による絞り込みより前に行う (assert.equal は asserts で型を狭める)
  assert.equal(parsed.type, 'pane.exited');
});

test('受信側の未知の enum 値は "unknown" として読む', () => {
  const out = PaneOutputParamsSchema.parse({ seq: 1, ts: TS, paneId: PANE_ID, dataB64: 'AA==', replay: 'cells' });
  assert.equal(out.replay, 'unknown');
  const state = parseKnownEvent(
    EventParamsSchema.parse({ seq: 5, ts: TS, type: 'pane.state', paneId: PANE_ID, data: { state: 'waiting', decidedBy: 'hook', prev: 'napping' } }),
  );
  assert.deepEqual(state?.data, { state: 'unknown', decidedBy: 'hook', prev: 'unknown' });
  const input = parseKnownEvent(
    EventParamsSchema.parse({ seq: 6, ts: TS, type: 'input', paneId: PANE_ID, data: { source: 'api', bytes: 3 } }),
  );
  assert.deepEqual(input?.data, { source: 'unknown', bytes: 3 });
});

test('既知の type で data が合わなければ ZodError', () => {
  const ev = EventParamsSchema.parse({ seq: 3, ts: TS, type: 'pane.resized', paneId: PANE_ID, data: { cols: 0, rows: 24, clientId: null } });
  assert.throws(() => parseKnownEvent(ev));
});

test('Object のプロトタイプ名は既知の type として扱わない', () => {
  const ev = EventParamsSchema.parse({ seq: 4, ts: TS, type: 'constructor', data: {} });
  assert.equal(parseKnownEvent(ev), undefined);
});

test('既知のイベントがすべて定義されている', () => {
  assert.equal(Object.keys(knownEventData).length, 18);
});
