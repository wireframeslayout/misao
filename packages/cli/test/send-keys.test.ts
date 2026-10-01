import assert from 'node:assert/strict';
import { test } from 'node:test';
import { CliError } from '../src/errors.js';
import { encodeKeys } from '../src/send-keys.js';

test('encodeKeys: 名前付きキーと C-x を大文字小文字を問わずバイト列にする', () => {
  assert.deepEqual(encodeKeys(['Enter', 'escape', 'C-c', 'c-d', 'Up']), Buffer.from('\r\x1b\x03\x04\x1b[A'));
});

test('encodeKeys: 不明なキー名は usage エラー', () => {
  for (const name of ['Bogus', 'C-[', 'c-']) {
    assert.throws(() => encodeKeys([name]), (e: unknown) => e instanceof CliError && e.kind === 'usage');
  }
});
