import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  MAX_SOCKET_PATH_BYTES,
  MisaoPathError,
  resolveMisaoDirs,
  resolveSocketPath,
} from '../src/socket-path.js';

const homeDir = '/home/u';

test('既定は ~/.misao/misao.sock', () => {
  assert.equal(resolveSocketPath({ env: {}, homeDir }), '/home/u/.misao/misao.sock');
});

test('$MISAO_DIR/misao.sock', () => {
  assert.equal(
    resolveSocketPath({ env: { MISAO_DIR: '/var/m' }, homeDir }),
    '/var/m/misao.sock',
  );
});

test('explicitPath は $MISAO_DIR に勝つ', () => {
  assert.equal(
    resolveSocketPath({ env: { MISAO_DIR: '/var/m' }, explicitPath: '/x/a.sock', homeDir }),
    '/x/a.sock',
  );
});

test('$MISAO_SOCKET は explicitPath と $MISAO_DIR に勝つ', () => {
  assert.equal(
    resolveSocketPath({
      env: { MISAO_SOCKET: '/e/s.sock', MISAO_DIR: '/var/m' },
      explicitPath: '/x/a.sock',
      homeDir,
    }),
    '/e/s.sock',
  );
});

test('空文字の env は未設定扱い', () => {
  assert.equal(
    resolveSocketPath({
      env: { MISAO_SOCKET: '', MISAO_DIR: '' },
      explicitPath: '/x/a.sock',
      homeDir,
    }),
    '/x/a.sock',
  );
  assert.equal(
    resolveSocketPath({ env: { MISAO_SOCKET: '', MISAO_DIR: '' }, homeDir }),
    '/home/u/.misao/misao.sock',
  );
});

test('~/ は homeDir に展開される', () => {
  assert.equal(resolveSocketPath({ env: {}, explicitPath: '~/s/a.sock', homeDir }), '/home/u/s/a.sock');
  assert.equal(resolveSocketPath({ env: { MISAO_SOCKET: '~/b.sock' }, homeDir }), '/home/u/b.sock');
  assert.equal(
    resolveSocketPath({ env: { MISAO_DIR: '~/d' }, homeDir }),
    '/home/u/d/misao.sock',
  );
});

test('相対パスは MisaoPathError', () => {
  assert.throws(
    () => resolveSocketPath({ env: {}, explicitPath: 'rel/a.sock', homeDir }),
    MisaoPathError,
  );
  assert.throws(
    () => resolveSocketPath({ env: { MISAO_SOCKET: './a.sock' }, homeDir }),
    MisaoPathError,
  );
  assert.throws(
    () => resolveSocketPath({ env: { MISAO_DIR: 'rel' }, homeDir }),
    MisaoPathError,
  );
});

function pathOfBytes(bytes: number, filler = 'a'): string {
  const prefix = '/';
  const suffix = '.sock';
  const fillerBytes = Buffer.byteLength(filler);
  const count = Math.floor((bytes - prefix.length - suffix.length) / fillerBytes);
  const base = prefix + filler.repeat(count) + suffix;
  const pad = bytes - Buffer.byteLength(base);
  return prefix + 'a'.repeat(pad) + filler.repeat(count) + suffix;
}

test('長さ境界: 107 バイトは OK、108 バイトは NG', () => {
  const ok = pathOfBytes(MAX_SOCKET_PATH_BYTES);
  assert.equal(Buffer.byteLength(ok), 107);
  assert.equal(resolveSocketPath({ env: {}, explicitPath: ok, homeDir }), ok);
  const ng = pathOfBytes(MAX_SOCKET_PATH_BYTES + 1);
  assert.equal(Buffer.byteLength(ng), 108);
  assert.throws(() => resolveSocketPath({ env: {}, explicitPath: ng, homeDir }), MisaoPathError);
});

test('長さはバイト数で数える（マルチバイト）', () => {
  const ok = pathOfBytes(MAX_SOCKET_PATH_BYTES, 'あ');
  assert.equal(Buffer.byteLength(ok), 107);
  assert.ok(ok.length < 107);
  assert.equal(resolveSocketPath({ env: {}, explicitPath: ok, homeDir }), ok);
  const ng = pathOfBytes(MAX_SOCKET_PATH_BYTES + 1, 'あ');
  assert.equal(Buffer.byteLength(ng), 108);
  assert.throws(() => resolveSocketPath({ env: {}, explicitPath: ng, homeDir }), MisaoPathError);
});

test('resolveMisaoDirs: $MISAO_DIR（~/ 展開）> ~/.misao の順、空文字は未設定、相対は MisaoPathError', () => {
  const fallback = { path: '/home/u/.misao', origin: 'default' };
  assert.deepEqual(resolveMisaoDirs({ env: {}, homeDir }), [fallback]);
  assert.deepEqual(resolveMisaoDirs({ env: { MISAO_DIR: '' }, homeDir }), [fallback]);
  assert.deepEqual(resolveMisaoDirs({ env: { MISAO_DIR: '~/d' }, homeDir }), [
    { path: '/home/u/d', origin: 'MISAO_DIR' },
    fallback,
  ]);
  assert.throws(() => resolveMisaoDirs({ env: { MISAO_DIR: 'rel' }, homeDir }), MisaoPathError);
});
