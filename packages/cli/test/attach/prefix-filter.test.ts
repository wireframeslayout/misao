import assert from 'node:assert/strict';
import { test } from 'node:test';
import { PrefixFilter } from '../../src/attach/prefix-filter.js';
import type { KeyBindings } from '../../src/config/index.js';

const KEYS: KeyBindings = { prefix: 0x1e, detach: 0x64, next: 0x6e, prev: 0x70, list: 0x6c };
const PREFIX = Buffer.from([0x1e]);

function feed(filter: PrefixFilter, ...parts: Array<string | Buffer>): Array<{ forward: string; action: string | null }> {
  return parts.map((p) => {
    const r = filter.feed(typeof p === 'string' ? Buffer.from(p) : p);
    return { forward: r.forward.toString('utf8'), action: r.action };
  });
}

test('prefix の無い入力はそのまま転送する', () => {
  assert.deepEqual(feed(new PrefixFilter(KEYS), 'hello dnpl'), [{ forward: 'hello dnpl', action: null }]);
});

test('prefix + d / n / p / l はアクションになり、手前までを転送する', () => {
  for (const [key, action] of [['d', 'detach'], ['n', 'next'], ['p', 'prev'], ['l', 'list']] as const) {
    const r = feed(new PrefixFilter(KEYS), Buffer.concat([Buffer.from('ab'), PREFIX, Buffer.from(`${key}zz`)]));
    assert.deepEqual(r, [{ forward: 'ab', action }], key);
  }
});

test('prefix を 2 回で prefix の 1 バイトを送り、その他のキーは捨てる', () => {
  const f = new PrefixFilter(KEYS);
  assert.deepEqual(feed(f, Buffer.concat([PREFIX, PREFIX, Buffer.from('x')])), [{ forward: '\x1ex', action: null }]);
  assert.deepEqual(feed(f, Buffer.concat([Buffer.from('a'), PREFIX, Buffer.from('zb')])), [{ forward: 'ab', action: null }]);
});

test('prefix と次のキーが別のチャンクで届いても同じ', () => {
  const f = new PrefixFilter(KEYS);
  assert.deepEqual(feed(f, PREFIX, 'd'), [
    { forward: '', action: null },
    { forward: '', action: 'detach' },
  ]);
});

test('設定したキーに従う (prefix=Ctrl-A, detach=q)', () => {
  const f = new PrefixFilter({ ...KEYS, prefix: 0x01, detach: 0x71 });
  assert.deepEqual(feed(f, 'd', Buffer.from([0x01, 0x71])), [
    { forward: 'd', action: null },
    { forward: '', action: 'detach' },
  ]);
  assert.deepEqual(feed(f, Buffer.from([0x1e])), [{ forward: '\x1e', action: null }], '既定の prefix は普通のキー');
});
