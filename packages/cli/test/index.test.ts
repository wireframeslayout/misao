import assert from 'node:assert/strict';
import { test } from 'node:test';
import { PACKAGE } from '../src/index.js';

test('exports the package name', () => {
  assert.equal(PACKAGE, 'misao');
});
