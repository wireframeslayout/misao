import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolveConfigCandidates } from '../../src/config/paths.js';

const homeDir = '/home/u';

test('env も flag も無ければ ~/.misao/misao.json のみ（暗黙）', () => {
  assert.deepEqual(resolveConfigCandidates({ env: {}, homeDir }), [
    { path: '/home/u/.misao/misao.json', origin: 'default' },
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
      { path: '/f.json', origin: '--config' },
      { path: '/c.json', origin: 'MISAO_CONFIG' },
      { path: '/d/misao.json', origin: 'MISAO_DIR' },
      { path: '/home/u/.misao/misao.json', origin: 'default' },
    ],
  );
});

test('空文字の env は未設定扱い', () => {
  assert.deepEqual(resolveConfigCandidates({ env: { MISAO_CONFIG: '', MISAO_DIR: '' }, homeDir }), [
    { path: '/home/u/.misao/misao.json', origin: 'default' },
  ]);
});

test('$MISAO_DIR の ~/ は homeDir に展開される', () => {
  assert.deepEqual(resolveConfigCandidates({ env: { MISAO_DIR: '~/d' }, homeDir }), [
    { path: '/home/u/d/misao.json', origin: 'MISAO_DIR' },
    { path: '/home/u/.misao/misao.json', origin: 'default' },
  ]);
});
