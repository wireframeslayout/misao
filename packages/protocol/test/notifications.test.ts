import assert from 'node:assert/strict';
import { test } from 'node:test';
import { knownEventData, parseKnownEvent } from '../src/events.js';
import { EventParamsSchema, PaneLineParamsSchema, PaneOutputParamsSchema } from '../src/notifications.js';
import { PANE_ID, TS } from './fixtures.js';

test('pane.output / pane.line / event の通知', () => {
  const out = PaneOutputParamsSchema.parse({ seq: 1, ts: TS, paneId: PANE_ID, dataB64: 'AA==', replay: 'snapshot' });
  assert.equal(out.replay, 'snapshot');
  assert.equal(PaneOutputParamsSchema.safeParse({ seq: 1, ts: TS, paneId: PANE_ID, dataB64: 'AA==', replay: 'raw' }).success, false);
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
  assert.equal(parsed?.type, 'pane.exited');
  assert.equal(parsed?.paneId, PANE_ID);
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
