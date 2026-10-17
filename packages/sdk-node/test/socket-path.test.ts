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

// ---- win32: 名前付きパイプ ----

const win = { platform: 'win32', homeDir: 'C:\\Users\\u', userName: 'u' } as const;

test('win32: 既定は \\\\.\\pipe\\misao-<user>-<hash(~/.misao)>', () => {
  const pipe = resolveSocketPath({ env: {}, ...win });
  assert.match(pipe, /^\\\\\.\\pipe\\misao-u-[0-9a-f]{16}$/);
  assert.equal(resolveSocketPath({ env: {}, ...win }), pipe);
});

test('win32: $MISAO_DIR が違えば名前も違い、同じなら（大文字小文字を除いて）同じ', () => {
  const a = resolveSocketPath({ env: { MISAO_DIR: 'C:\\misao-a' }, ...win });
  const b = resolveSocketPath({ env: { MISAO_DIR: 'C:\\misao-b' }, ...win });
  const aUpper = resolveSocketPath({ env: { MISAO_DIR: 'C:\\MISAO-A' }, ...win });
  assert.notEqual(a, b);
  assert.equal(a, aUpper);
  assert.notEqual(a, resolveSocketPath({ env: {}, ...win }));
});

test('win32: ユーザー名はパイプ名に使える文字へ寄せ、長さを切る', () => {
  const pipe = resolveSocketPath({ env: {}, ...win, userName: 'DOMAIN\\ユーザー name' });
  assert.match(pipe, /^\\\\\.\\pipe\\misao-DOMAIN__+name-[0-9a-f]{16}$/);
  const long = resolveSocketPath({ env: {}, ...win, userName: 'x'.repeat(100) });
  assert.match(long, /^\\\\\.\\pipe\\misao-x{32}-[0-9a-f]{16}$/);
});

test('win32: $MISAO_SOCKET / explicitPath はパイプ名をそのまま使い、優先順は同じ', () => {
  assert.equal(
    resolveSocketPath({ env: { MISAO_SOCKET: '\\\\.\\pipe\\e' }, explicitPath: '\\\\.\\pipe\\x', ...win }),
    '\\\\.\\pipe\\e',
  );
  assert.equal(resolveSocketPath({ env: {}, explicitPath: '\\\\.\\pipe\\x', ...win }), '\\\\.\\pipe\\x');
});

test('win32: パイプ名でない指定は MisaoPathError（Unix ソケットのパスを黙って使わない）', () => {
  assert.throws(() => resolveSocketPath({ env: { MISAO_SOCKET: 'C:\\s\\a.sock' }, ...win }), MisaoPathError);
  assert.throws(() => resolveSocketPath({ env: {}, explicitPath: '/tmp/a.sock', ...win }), MisaoPathError);
});

test('win32: パイプ名は 256 文字まで（Unix の 107 バイト制限は適用しない）', () => {
  const prefix = '\\\\.\\pipe\\';
  const ok = prefix + 'a'.repeat(256 - prefix.length);
  assert.equal(resolveSocketPath({ env: {}, explicitPath: ok, ...win }), ok);
  assert.throws(() => resolveSocketPath({ env: {}, explicitPath: ok + 'a', ...win }), MisaoPathError);
});

test('win32: resolveMisaoDirs は Windows のパス規則で ~\\ を展開する', () => {
  assert.deepEqual(resolveMisaoDirs({ env: { MISAO_DIR: '~\\d' }, homeDir: 'C:\\Users\\u', platform: 'win32' }), [
    { path: 'C:\\Users\\u\\d', origin: 'MISAO_DIR' },
    { path: 'C:\\Users\\u\\.misao', origin: 'default' },
  ]);
});

test('非 win32 の platform ではパイプ名を受け付けない（相対パス扱いで MisaoPathError）', () => {
  assert.throws(
    () => resolveSocketPath({ env: {}, explicitPath: '\\\\.\\pipe\\x', homeDir, platform: 'linux' }),
    MisaoPathError,
  );
});

test('win32: 接頭辞だけ・NUL や制御文字・区切りを含むパイプ名は MisaoPathError', () => {
  const prefix = '\\\\.\\pipe\\';
  for (const bad of [prefix, `${prefix}a\0b`, `${prefix}a\nb`, `${prefix}a\\b`, `${prefix}a/b`]) {
    assert.throws(() => resolveSocketPath({ env: { MISAO_SOCKET: bad }, ...win }), MisaoPathError, JSON.stringify(bad));
    assert.throws(() => resolveSocketPath({ env: {}, explicitPath: bad, ...win }), MisaoPathError, JSON.stringify(bad));
  }
});
