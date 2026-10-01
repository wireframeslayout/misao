import { test } from 'node:test';
import assert from 'node:assert/strict';
import { TitleActivity } from '../src/title-activity.js';

test('タイトルが無ければ意見なし', () => {
  assert.equal(new TitleActivity().verdict(0), null);
});

test('点字・◐◑・✻ などで始まるタイトルは working (交互に切り替わっても working)', () => {
  const t = new TitleActivity();
  for (const [i, glyph] of ['◐', '◑', '⠋', '⠙', '✻', '∗'].entries()) {
    t.setTitle(`${glyph} task`, i * 1000);
    assert.equal(t.verdict(i * 1000), 'working', glyph);
  }
});

test('スピナーを見た後の ✳ のような空でない非スピナーのタイトルは idle', () => {
  const t = new TitleActivity();
  t.setTitle('◐ task', 0);
  t.setTitle('✳ task', 1000);
  assert.equal(t.verdict(1000), 'idle');
});

test('スピナーを見る前の ✳ は意見なし', () => {
  const t = new TitleActivity();
  t.setTitle('✳ task', 0);
  assert.equal(t.verdict(0), null);
});

test('スピナーを見た後でも、空のタイトルは意見なし', () => {
  const t = new TitleActivity();
  t.setTitle('◐ task', 0);
  t.setTitle('', 500);
  assert.equal(t.verdict(500), null);
});

test('スピナーのタイトルが 3 秒更新されなければ意見なしになり、更新されたら working に戻る', () => {
  const t = new TitleActivity();
  t.setTitle('◐ task', 1000);
  assert.equal(t.verdict(3999), 'working');
  assert.equal(t.verdict(4000), null);
  t.setTitle('◑ task', 4500);
  assert.equal(t.verdict(4500), 'working');
});
