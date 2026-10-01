import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readLoginShell } from '../src/cli-io.js';

test('readLoginShell: シェルを返し、無いか取れなければ null', () => {
  assert.equal(readLoginShell(() => ({ shell: '/bin/bash' })), '/bin/bash');
  assert.equal(readLoginShell(() => ({ shell: null })), null);
  assert.equal(
    readLoginShell(() => {
      throw new Error('ENOENT: no such file or directory, uv_os_get_passwd');
    }),
    null,
  );
});
