import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatKeySpec, parseKeySpec, parsePrefixKeySpec } from '../../src/config/keys.js';

test('C-^ は 0x1e', () => {
  assert.equal(parseKeySpec('C-^'), 0x1e);
});

test('C-<ch> は制御コード（大文字小文字を区別しない）', () => {
  assert.equal(parseKeySpec('C-a'), 0x01);
  assert.equal(parseKeySpec('C-A'), 0x01);
  assert.equal(parseKeySpec('C-z'), 0x1a);
  assert.equal(parseKeySpec('C-@'), 0x00);
  assert.equal(parseKeySpec('C-\\'), 0x1c);
  assert.equal(parseKeySpec('C-]'), 0x1d);
  assert.equal(parseKeySpec('C-_'), 0x1f);
});

test('単一の印字可能 ASCII は 1 バイト', () => {
  assert.equal(parseKeySpec('d'), 0x64);
  assert.equal(parseKeySpec(' '), 0x20);
  assert.equal(parseKeySpec('~'), 0x7e);
});

test('C-[ は拒否', () => {
  assert.throws(() => parseKeySpec('C-['), /ESC/);
});

test('不正入力は理由付きエラー', () => {
  for (const bad of ['', 'ab', 'C-', 'C-1', 'C-ab', 'あ', '\x7f', '\n', 'c-a']) {
    assert.throws(() => parseKeySpec(bad), /invalid key spec/, JSON.stringify(bad));
  }
});

test('prefix は制御キーのみ', () => {
  assert.equal(parsePrefixKeySpec('C-^'), 0x1e);
  assert.equal(parsePrefixKeySpec('C-a'), 0x01);
  for (const bad of ['a', ' ', 'd']) {
    assert.throws(() => parsePrefixKeySpec(bad), /prefix must be a control key/, JSON.stringify(bad));
  }
  assert.throws(() => parsePrefixKeySpec('C-['), /ESC/);
});

test('formatKeySpec: 制御キーは Ctrl-<char>、印字可能文字はそのまま', () => {
  assert.equal(formatKeySpec(0x1e), 'Ctrl-^');
  assert.equal(formatKeySpec(0x1d), 'Ctrl-]');
  assert.equal(formatKeySpec(0x01), 'Ctrl-A');
  assert.equal(formatKeySpec(0x00), 'Ctrl-@');
  assert.equal(formatKeySpec(0x64), 'd');
  assert.equal(formatKeySpec(parseKeySpec('C-^')), 'Ctrl-^');
  assert.throws(() => formatKeySpec(0x7f), RangeError);
});
