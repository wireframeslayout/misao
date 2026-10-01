import assert from 'node:assert/strict';
import { test } from 'node:test';
import { displayWidth, renderTable } from '../../src/view/table.js';

test('displayWidth: 半角 1、全角 2', () => {
  assert.equal(displayWidth('abc'), 3);
  assert.equal(displayWidth('表示名'), 6);
  assert.equal(displayWidth('(未登録) a'), 10);
  assert.equal(displayWidth(''), 0);
});

test('renderTable: 全角を 2 桁として列をそろえ、最後の列は埋めない', () => {
  const lines = renderTable([
    ['NAME', 'LAST'],
    ['表示名', '1s'],
    ['ab', '2m'],
  ]);
  assert.deepEqual(lines, ['NAME    LAST', '表示名  1s', 'ab      2m']);
});

test('renderTable: 空なら空', () => {
  assert.deepEqual(renderTable([]), []);
});
