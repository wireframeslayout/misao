import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SizeArbiter } from '../src/size-arbiter.js';

test('record: 最後に操作したクライアントが所有者になる', () => {
  const a = new SizeArbiter();
  assert.deepEqual(a.record('A', 100, 30), { cols: 100, rows: 30, owner: 'A' });
  assert.deepEqual(a.record('B', 60, 20), { cols: 60, rows: 20, owner: 'B' });
  assert.equal(a.owner, 'B');
});

test('record: clientId なしは所有者を null にする', () => {
  const a = new SizeArbiter();
  a.record('A', 100, 30);
  assert.deepEqual(a.record(null, 80, 24), { cols: 80, rows: 24, owner: null });
  assert.equal(a.owner, null);
});

test('claim: 記録済みのサイズへ戻す。所有者本人・未記録は null', () => {
  const a = new SizeArbiter();
  a.record('A', 100, 30);
  a.record('B', 60, 20);
  assert.deepEqual(a.claim('A'), { cols: 100, rows: 30, owner: 'A' });
  assert.equal(a.claim('A'), null);
  assert.equal(a.claim('unknown'), null);
});

test('forget: 所有者が抜けたら、残りのうち最後に操作したクライアントのサイズを返す', () => {
  const a = new SizeArbiter();
  a.record('A', 100, 30);
  a.record('B', 60, 20);
  a.record('C', 90, 25);
  assert.deepEqual(a.forget('C'), { cols: 60, rows: 20, owner: 'B' });
  assert.equal(a.owner, 'B');
});

test('forget: 所有者でなければ null で所有者は変わらない。最後の 1 人が抜けたら所有者なし', () => {
  const a = new SizeArbiter();
  a.record('A', 100, 30);
  a.record('B', 60, 20);
  assert.equal(a.forget('A'), null);
  assert.equal(a.owner, 'B');
  assert.equal(a.forget('B'), null);
  assert.equal(a.owner, null);
});
