import assert from 'node:assert/strict';
import { test } from 'node:test';
import { NAMED_PIPE_PREFIX, isNamedPipePath } from '../src/endpoint.js';

test('isNamedPipePath: \\\\.\\pipe\\ で始まるものだけ true（大文字小文字・/ 区切りも許す）', () => {
  assert.equal(isNamedPipePath(`${NAMED_PIPE_PREFIX}misao-u-abc`), true);
  assert.equal(isNamedPipePath('\\\\.\\PIPE\\misao'), true);
  assert.equal(isNamedPipePath('//./pipe/misao'), true);
  assert.equal(isNamedPipePath('/home/u/.misao/misao.sock'), false);
  assert.equal(isNamedPipePath('C:\\Users\\u\\misao.sock'), false);
  assert.equal(isNamedPipePath('\\\\server\\pipe\\misao'), false);
  assert.equal(isNamedPipePath(''), false);
});
