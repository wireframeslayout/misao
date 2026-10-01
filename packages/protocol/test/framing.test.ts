import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DEFAULT_MAX_LINE_BYTES, LineSplitter, encodeMessage } from '../src/framing.js';

const b = (s: string): Buffer => Buffer.from(s);

test('LineSplitter: 1 チャンクに複数メッセージ', () => {
  const s = new LineSplitter();
  assert.deepEqual(s.push(b('{"a":1}\n{"b":2}\n')), ['{"a":1}', '{"b":2}']);
});

test('LineSplitter: メッセージが複数チャンクに分割', () => {
  const s = new LineSplitter();
  assert.deepEqual(s.push(b('{"a":')), []);
  assert.deepEqual(s.push(b('1}\n{"b"')), ['{"a":1}']);
  assert.deepEqual(s.push(b(':2}\n')), ['{"b":2}']);
});

test('LineSplitter: UTF-8 マルチバイトがチャンク境界で切れても壊れない', () => {
  const s = new LineSplitter();
  const bytes = Buffer.from('{"t":"日本語🎉"}\n');
  const got: string[] = [];
  for (const b of bytes) got.push(...s.push(Buffer.from([b])));
  assert.deepEqual(got, ['{"t":"日本語🎉"}']);
});

test('LineSplitter: 空行は無視', () => {
  assert.deepEqual(new LineSplitter().push(b('\n\n{"a":1}\n\n')), ['{"a":1}']);
});

test('LineSplitter: 1 行が上限を超えたら例外', () => {
  const s = new LineSplitter(4);
  assert.deepEqual(s.push(b('abcd\n')), ['abcd']);
  assert.deepEqual(s.push(b('ab')), []);
  assert.throws(() => s.push(b('cde')), /exceeds 4 bytes/);
});

test('LineSplitter: 上限は 1 行ごとで、改行を含むチャンク全体には掛からない', () => {
  const s = new LineSplitter(4);
  assert.deepEqual(s.push(b('abcd\nefgh\nij')), ['abcd', 'efgh']);
  assert.deepEqual(s.push(b('\n')), ['ij']);
});

test('LineSplitter: 既定の上限は 8 MiB', () => {
  assert.equal(DEFAULT_MAX_LINE_BYTES, 8 * 1024 * 1024);
});

test('encodeMessage: 末尾改行つき 1 行', () => {
  const line = encodeMessage({ jsonrpc: '2.0', id: 1, result: 'a\nb' });
  assert.equal(line.split('\n').length, 2);
  assert.ok(line.endsWith('\n'));
});
