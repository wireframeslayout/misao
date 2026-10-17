import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readLoginShell } from '../src/cli-io.js';
import { loginShell } from '../src/open-pane.js';

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

test('readLoginShell: win32 は passwd を見ず %COMSPEC%、無ければ null', () => {
  const unused = (): { shell: string | null } => {
    throw new Error('must not be called');
  };
  assert.equal(readLoginShell(unused, 'win32', { COMSPEC: 'C:\\Windows\\System32\\cmd.exe' }), 'C:\\Windows\\System32\\cmd.exe');
  assert.equal(readLoginShell(unused, 'win32', {}), null);
  assert.equal(readLoginShell(unused, 'win32', { COMSPEC: '' }), null);
});

test('loginShell: Unix は -l を付け、Windows の cmd.exe には付けない。シェルが無ければ usage エラー', () => {
  assert.deepEqual(loginShell('/bin/bash', 'linux'), ['/bin/bash', '-l']);
  assert.deepEqual(loginShell('C:\\Windows\\System32\\cmd.exe', 'win32'), ['C:\\Windows\\System32\\cmd.exe']);
  assert.throws(() => loginShell(null, 'win32'), /ログインシェルを特定できません/);
});
