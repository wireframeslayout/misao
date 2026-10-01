import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ByteActivity } from '../src/byte-activity.js';

test('開いてから静かなままなら 5 秒で idle になり、それまでは意見なし', () => {
  const b = new ByteActivity(0);
  assert.equal(b.tick(1000), null);
  assert.equal(b.tick(4000), null);
  assert.equal(b.tick(5000), 'idle');
});

test('しきい値 (3 秒で 200B) 未満の出力では working にならない', () => {
  const b = new ByteActivity(0);
  b.record(199, 100);
  assert.equal(b.tick(1000), null);
  b.record(1, 1100); // 合計 200B になっても、連続 2 ティック必要
  assert.equal(b.tick(2000), null);
});

test('200B 以上のティックが 2 回続いたら working、1 回だけなら working にならない', () => {
  const b = new ByteActivity(0);
  b.record(300, 100);
  assert.equal(b.tick(1000), null); // 1 回目
  assert.equal(b.tick(2000), null); // 新しい出力が無いので連続が切れる
  b.record(300, 2500);
  assert.equal(b.tick(3000), null);
  b.record(10, 3500);
  assert.equal(b.tick(4000), 'working'); // ウィンドウ内の合計が 200B 以上のティックが 2 回続いた
});

test('working は、最後の活動のティックから 5 秒静かになったら idle に戻り、出力が続く間は戻らない', () => {
  const b = new ByteActivity(0);
  b.record(300, 100);
  b.tick(1000);
  b.record(300, 1100);
  assert.equal(b.tick(2000), 'working');
  b.record(300, 2100);
  assert.equal(b.tick(3000), 'working'); // lastAbove = 3000
  assert.equal(b.tick(7999), 'working');
  assert.equal(b.tick(8000), 'idle');
});

test('入力の後 500ms の出力は数えない', () => {
  const b = new ByteActivity(0);
  b.notifyInput(1000);
  b.record(500, 1499);
  assert.equal(b.tick(2000), null);
  b.record(500, 1500); // 猶予が切れた直後は数える
  b.tick(2500);
  b.record(500, 2600);
  assert.equal(b.tick(3000), 'working');
});

test('resize の後 800ms の出力は数えない', () => {
  const b = new ByteActivity(0);
  b.notifyResize(1000);
  b.record(500, 1799);
  assert.equal(b.tick(2000), null);
  b.record(500, 1800);
  b.tick(2500);
  b.record(500, 2600);
  assert.equal(b.tick(3000), 'working');
});

test('idle からも、活動のティックが 2 回続けば working に戻る', () => {
  const b = new ByteActivity(0);
  assert.equal(b.tick(5000), 'idle');
  b.record(300, 5100);
  b.tick(6000);
  b.record(300, 6100);
  assert.equal(b.tick(7000), 'working');
});
