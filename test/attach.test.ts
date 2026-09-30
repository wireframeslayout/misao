import { test } from 'node:test';
import assert from 'node:assert/strict';
import { PrefixFilter, TTY_RESET } from '../src/cli/attach.js';

const feedAll = (f: PrefixFilter, ...chunks: number[][]) => chunks.map((c) => f.feed(Buffer.from(c)));

test('PrefixFilter: 通常入力はそのまま転送', () => {
  const r = new PrefixFilter().feed(Buffer.from('abc'));
  assert.equal(r.forward.toString(), 'abc');
  assert.equal(r.detach, false);
});
test('PrefixFilter: Ctrl-] d で detach (それ以前の入力は転送)', () => {
  const r = new PrefixFilter().feed(Buffer.from([0x61, 0x1d, 0x64, 0x62]));
  assert.equal(r.forward.toString(), 'a');
  assert.equal(r.detach, true);
});
test('PrefixFilter: Ctrl-] Ctrl-] はリテラル 0x1d を 1 つ送る', () => {
  const r = new PrefixFilter().feed(Buffer.from([0x1d, 0x1d]));
  assert.deepEqual([...r.forward], [0x1d]);
  assert.equal(r.detach, false);
});
test('PrefixFilter: prefix の後の他のキーは無視 (キー自体も転送しない)', () => {
  const r = new PrefixFilter().feed(Buffer.from([0x1d, 0x78, 0x79]));
  assert.equal(r.forward.toString(), 'y');
});
test('PrefixFilter: prefix と次のバイトがチャンクをまたいでも動く', () => {
  const f = new PrefixFilter();
  const [a, b] = feedAll(f, [0x1d], [0x64]);
  assert.equal(a!.detach, false);
  assert.equal(b!.detach, true);
});
test('TTY_RESET: alt screen / マウス / bracketed paste / カーソル表示を戻す', () => {
  for (const seq of ['?1049l', '?1000l', '?1002l', '?1003l', '?1006l', '?2004l', '?1l', '?25h']) {
    assert.ok(TTY_RESET.includes(seq), seq);
  }
});
