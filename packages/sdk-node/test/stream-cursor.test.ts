import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EVENTS_KEY, StreamCursor, linesKey } from '../src/stream-cursor.js';
import type { EventsEntry, LinesEntry } from '../src/stream-cursor.js';
import { PANE_ID, ulid } from './helpers/fake-daemon.js';

const events = (lastSeq = 0): EventsEntry => ({ kind: 'events', lastSeq, handler: () => undefined });
const lines = (lastSeq = 0): LinesEntry => ({ kind: 'lines', paneId: PANE_ID, lastSeq, handler: () => undefined });

test('accept advances lastSeq and drops duplicates', () => {
  const cursor = new StreamCursor();
  cursor.add(events());
  assert.equal(cursor.accept(EVENTS_KEY, 1), true);
  assert.equal(cursor.accept(EVENTS_KEY, 1), false);
  assert.equal(cursor.accept(EVENTS_KEY, 3), true);
  assert.equal(cursor.accept(EVENTS_KEY, 2), false);
  assert.equal(cursor.get(EVENTS_KEY)?.lastSeq, 3);
});

test('accept ignores streams that are not registered', () => {
  assert.equal(new StreamCursor().accept(linesKey(PANE_ID), 1), false);
});

test('adding the same stream twice throws', () => {
  const cursor = new StreamCursor();
  cursor.add(lines());
  assert.throws(() => cursor.add(lines()), /already registered/);
});

test('remove only drops the entry that is registered', () => {
  const cursor = new StreamCursor();
  const first = events();
  cursor.add(first);
  cursor.remove(events());
  assert.equal(cursor.has(EVENTS_KEY), true);
  cursor.remove(first);
  assert.equal(cursor.has(EVENTS_KEY), false);
});

test('the first epoch resets nothing; a different epoch resets every stream to 0', () => {
  const cursor = new StreamCursor();
  cursor.add(events(7));
  cursor.add(lines(9));
  assert.deepEqual(cursor.resetForEpoch(ulid(1)), []);
  assert.equal(cursor.get(EVENTS_KEY)?.lastSeq, 7);
  assert.deepEqual(cursor.resetForEpoch(ulid(1)), []);
  assert.equal(cursor.resetForEpoch(ulid(2)).length, 2);
  assert.equal(cursor.get(EVENTS_KEY)?.lastSeq, 0);
  assert.equal(cursor.get(linesKey(PANE_ID))?.lastSeq, 0);
  assert.equal(cursor.epoch, ulid(2));
});

test('resubscribe params carry lastSeq and the current epoch', () => {
  const cursor = new StreamCursor();
  const entry = events(4);
  cursor.add(entry);
  cursor.resetForEpoch(ulid(1));
  assert.deepEqual(cursor.resubscribeParams(entry), { since: 4, epoch: ulid(1) });
  assert.deepEqual(cursor.position(entry), { seq: 4, epoch: ulid(1) });
});

test('params need a known epoch', () => {
  const cursor = new StreamCursor();
  assert.throws(() => cursor.resubscribeParams(events()), /epoch/);
});
