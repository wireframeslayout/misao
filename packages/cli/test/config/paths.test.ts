import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolveConfigCandidates } from '../../src/config/paths.js';

const homeDir = '/home/u';

test('env も flag も無ければ ~/.misao/misao.json のみ（暗黙）', () => {
  assert.deepEqual(resolveConfigCandidates({ env: {}, homeDir }), [
    { path: '/home/u/.misao/misao.json', isExplicit: false },
  ]);
});

test('優先順: flag > $MISAO_CONFIG > $MISAO_DIR > ~/.misao', () => {
  assert.deepEqual(
    resolveConfigCandidates({
      flagPath: '/f.json',
      env: { MISAO_CONFIG: '/c.json', MISAO_DIR: '/d' },
      homeDir,
    }),
    [
      { path: '/f.json', isExplicit: true },
      { path: '/c.json', isExplicit: true },
      { path: '/d/misao.json', isExplicit: false },
      { path: '/home/u/.misao/misao.json', isExplicit: false },
    ],
  );
});

test('空文字の env は未設定扱い', () => {
  assert.deepEqual(resolveConfigCandidates({ env: { MISAO_CONFIG: '', MISAO_DIR: '' }, homeDir }), [
    { path: '/home/u/.misao/misao.json', isExplicit: false },
  ]);
});
