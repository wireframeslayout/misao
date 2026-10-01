import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  DimensionSchema,
  EpochSchema,
  PaneIdSchema,
  SeqSchema,
  SinceSchema,
  TsSchema,
  UlidSchema,
  WindowIdSchema,
} from '../src/primitives.js';
import { PANE_ID, TS, ULID, WINDOW_ID } from './fixtures.js';

test('PaneId は p_ + ULID', () => {
  assert.equal(PaneIdSchema.safeParse(PANE_ID).success, true);
  assert.equal(PaneIdSchema.safeParse(ULID).success, false);
  assert.equal(PaneIdSchema.safeParse(WINDOW_ID).success, false);
  assert.equal(PaneIdSchema.safeParse(`p_${ULID}X`).success, false);
});

test('WindowId は w_ + ULID', () => {
  assert.equal(WindowIdSchema.safeParse(WINDOW_ID).success, true);
  assert.equal(WindowIdSchema.safeParse(PANE_ID).success, false);
});

test('Epoch は ULID そのもの (I L O U は不可)', () => {
  assert.equal(EpochSchema.safeParse(ULID).success, true);
  assert.equal(UlidSchema.safeParse('I'.repeat(26)).success, false);
  assert.equal(EpochSchema.safeParse(ULID.slice(1)).success, false);
  assert.equal(UlidSchema.safeParse(`8${ULID.slice(1)}`).success, false, '先頭は 0-7');
  assert.equal(UlidSchema.safeParse(`7${'Z'.repeat(25)}`).success, true);
});

test('seq / since の境界値', () => {
  assert.equal(SeqSchema.safeParse(0).success, true);
  assert.equal(SeqSchema.safeParse(-1).success, false);
  assert.equal(SeqSchema.safeParse(1.5).success, false);
  assert.equal(SinceSchema.safeParse(-1).success, false);
  assert.equal(SinceSchema.safeParse(0).success, true);
});

test('ts は ISO 8601 UTC', () => {
  assert.equal(TsSchema.safeParse(TS).success, true);
  assert.equal(TsSchema.safeParse('yesterday').success, false);
  assert.equal(TsSchema.safeParse('2026-10-01T00:00:00+09:00').success, false);
});

test('dimension は正の整数', () => {
  assert.equal(DimensionSchema.safeParse(1).success, true);
  assert.equal(DimensionSchema.safeParse(0).success, false);
});
