import assert from 'node:assert/strict';
import { test } from 'node:test';
import { LineSplitter, encodeMessage } from '../src/framing.js';

test('LineSplitter: 1 チャンクに複数メッセージ', () => {
  const s = new LineSplitter();
  assert.deepEqual(s.push('{"a":1}\n{"b":2}\n'), ['{"a":1}', '{"b":2}']);
});

test('LineSplitter: メッセージが複数チャンクに分割', () => {
  const s = new LineSplitter();
  assert.deepEqual(s.push('{"a":'), []);
  assert.deepEqual(s.push('1}\n{"b"'), ['{"a":1}']);
  assert.deepEqual(s.push(':2}\n'), ['{"b":2}']);
});

test('LineSplitter: UTF-8 マルチバイトがチャンク境界で切れても壊れない', () => {
  const s = new LineSplitter();
  const bytes = Buffer.from('{"t":"日本語🎉"}\n');
  const got: string[] = [];
  for (const b of bytes) got.push(...s.push(Buffer.from([b])));
  assert.deepEqual(got, ['{"t":"日本語🎉"}']);
});

test('LineSplitter: 空行は無視', () => {
  assert.deepEqual(new LineSplitter().push('\n\n{"a":1}\n\n'), ['{"a":1}']);
});

test('encodeMessage: 末尾改行つき 1 行', () => {
  const line = encodeMessage({ jsonrpc: '2.0', id: 1, result: 'a\nb' });
  assert.equal(line.split('\n').length, 2);
  assert.ok(line.endsWith('\n'));
});
