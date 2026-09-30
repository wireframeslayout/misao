import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SeqRing } from '../src/util/ring.js';
import { ulid, newPaneId } from '../src/util/ulid.js';

const TS = '2026-01-01T00:00:00.000Z';

test('seq は 1 始まりで単調増加', () => {
  const r = new SeqRing<string>(100);
  assert.equal(r.push('a', 1, TS), 1);
  assert.equal(r.push('b', 1, TS), 2);
  assert.equal(r.head, 2);
});

test('since: seq > since のみ返す', () => {
  const r = new SeqRing<number>(100);
  for (let i = 0; i < 5; i++) r.push(i, 1, TS);
  assert.deepEqual(r.since(0).map((e) => e.seq), [1, 2, 3, 4, 5]);
  assert.deepEqual(r.since(3).map((e) => e.seq), [4, 5]);
  assert.deepEqual(r.since(5), []);
  assert.deepEqual(r.since(99), []);
});

test('容量超過で古いものを捨て、oldest が進む', () => {
  const r = new SeqRing<number>(3);
  for (let i = 0; i < 5; i++) r.push(i, 1, TS);
  assert.equal(r.length, 3);
  assert.equal(r.oldest, 3);
  assert.deepEqual(r.since(0).map((e) => e.item), [2, 3, 4]);
});

test('gap: since が oldest - 1 より小さいときだけ true', () => {
  const r = new SeqRing<number>(3);
  for (let i = 0; i < 5; i++) r.push(i, 1, TS); // 保持 seq 3..5
  assert.equal(r.hasGap(1), true);
  assert.equal(r.hasGap(2), false); // 3 以降を全部持っている
  assert.equal(r.hasGap(5), false);
});

test('空リング: since === head なら gap なし、それ未満なら gap', () => {
  const r = new SeqRing<number>(10);
  assert.equal(r.hasGap(0), false);
  assert.equal(r.hasGap(7), true); // head より先 = 送信側の seq 巻き戻り
  const b = new SeqRing<number>(1);
  b.push(1, 5, TS); // 容量超過の 1 件は残る
  assert.equal(b.length, 1);
});

test('サイズ単位の容量: 大きな要素で複数追い出す', () => {
  const r = new SeqRing<string>(10);
  r.push('a', 4, TS);
  r.push('b', 4, TS);
  r.push('c', 8, TS);
  assert.deepEqual(r.since(0).map((e) => e.item), ['c']);
});

test('大量 push 後もコンパクションで since が正しい', () => {
  const r = new SeqRing<number>(50);
  for (let i = 0; i < 10000; i++) r.push(i, 1, TS);
  assert.equal(r.oldest, 9951);
  assert.deepEqual(r.since(9998).map((e) => e.seq), [9999, 10000]);
});

test('ULID: 26 文字 Crockford、時刻順にソート可能', () => {
  const ids = [ulid(1000), ulid(1001), ulid(2000), ulid(1_700_000_000_000)];
  for (const id of ids) assert.match(id, /^[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.deepEqual([...ids].sort(), ids);
});

test('ULID: 同一 ms でも単調増加・一意', () => {
  const ids = Array.from({ length: 1000 }, () => ulid(5000));
  assert.equal(new Set(ids).size, 1000);
  assert.deepEqual([...ids].sort(), ids);
  assert.match(newPaneId(), /^p_[0-9A-Z]{26}$/);
});
