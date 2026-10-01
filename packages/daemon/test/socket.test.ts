import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, symlinkSync, rmSync, statSync, writeFileSync } from 'node:fs';
import fsPromises from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, afterEach, before, mock, test } from 'node:test';
import { ensureSocketDir, listenUnixSocket, removeStaleSocket } from '../src/socket.js';

let root: string;
const originalUmask = process.umask();

before(() => {
  root = mkdtempSync(path.join(tmpdir(), 'misao-socket-'));
});

afterEach(() => {
  process.umask(originalUmask);
});

after(() => {
  process.umask(originalUmask);
  rmSync(root, { recursive: true, force: true });
});

const modeOf = (p: string): number => statSync(p).mode & 0o777;

test('ensureSocketDir: 無ければ 0o700 で作成（親も）', async () => {
  process.umask(0o022);
  const dir = path.join(root, 'a', 'b');
  await ensureSocketDir(path.join(dir, 'misao.sock'));
  assert.equal(modeOf(dir), 0o700);
});

test('ensureSocketDir: 既存の 0o700 ディレクトリは通る', async () => {
  const dir = path.join(root, 'existing');
  mkdirSync(dir, { recursive: true });
  chmodSync(dir, 0o700);
  await ensureSocketDir(path.join(dir, 'misao.sock'));
  assert.equal(modeOf(dir), 0o700);
});

test('ensureSocketDir: group 書き込み可のディレクトリは拒否し chmod 700 を案内', async () => {
  const dir = path.join(root, 'loose');
  await ensureSocketDir(path.join(dir, 'misao.sock'));
  chmodSync(dir, 0o770);
  await assert.rejects(ensureSocketDir(path.join(dir, 'misao.sock')), new RegExp(`chmod 700 ${dir}`));
});

test('ensureSocketDir: ディレクトリでないパスは拒否', async () => {
  const file = path.join(root, 'file');
  writeFileSync(file, '');
  await assert.rejects(ensureSocketDir(path.join(file, 'misao.sock')));
});

test('ensureSocketDir: シンボリックリンクは理由付きで拒否', async () => {
  const real = path.join(root, 'real');
  mkdirSync(real, { mode: 0o700 });
  const link = path.join(root, 'link');
  symlinkSync(real, link);
  await assert.rejects(ensureSocketDir(path.join(link, 'misao.sock')), /symbolic link/);
});

test('listenUnixSocket: umask 0o022 でもソケットは 0o600 になり、umask は変更しない', async () => {
  process.umask(0o022);
  const dir = path.join(root, 'sock');
  const socketPath = path.join(dir, 'misao.sock');
  await ensureSocketDir(socketPath);
  const server = createServer();
  try {
    await listenUnixSocket(server, socketPath);
    assert.equal(modeOf(socketPath), 0o600);
    assert.equal(modeOf(dir), 0o700);
    assert.equal(process.umask(), 0o022);
  } finally {
    server.close();
  }
});

test('listenUnixSocket: listen 失敗は reject し error リスナーを残さない', async () => {
  const server = createServer();
  await assert.rejects(listenUnixSocket(server, path.join(root, 'nonexistent-dir', 'x.sock')));
  assert.equal(server.listenerCount('error'), 0);
});

test('listenUnixSocket: listen が同期で例外を投げても error リスナーを残さない', async () => {
  const dir = path.join(root, 'twice');
  const socketPath = path.join(dir, 'misao.sock');
  await ensureSocketDir(socketPath);
  const server = createServer();
  try {
    await listenUnixSocket(server, socketPath);
    const before = server.listenerCount('error');
    await assert.rejects(listenUnixSocket(server, path.join(dir, 'other.sock')));
    assert.equal(server.listenerCount('error'), before);
  } finally {
    server.close();
  }
});

test('listenUnixSocket: chmod に失敗したら server を閉じて reject', async () => {
  const dir = path.join(root, 'chmod-fail');
  const socketPath = path.join(dir, 'misao.sock');
  await ensureSocketDir(socketPath);
  const server = createServer();
  const failure = new Error('chmod failed');
  mock.method(fsPromises, 'chmod', async () => {
    throw failure;
  });
  syncBuiltinESMExports();
  try {
    await assert.rejects(listenUnixSocket(server, socketPath), failure);
    assert.equal(server.listening, false);
  } finally {
    mock.restoreAll();
    syncBuiltinESMExports();
    server.close();
  }
});

test('removeStaleSocket: ファイルが無ければ何もしない', async () => {
  assert.equal(await removeStaleSocket(path.join(root, 'none.sock')), false);
});

test('removeStaleSocket: 誰も待ち受けていない (ECONNREFUSED) なら消す', async () => {
  const sock = path.join(root, 'stale.sock');
  writeFileSync(sock, ''); // ソケットでないファイルへの connect は ECONNREFUSED
  assert.equal(await removeStaleSocket(sock), true);
  assert.equal(existsSync(sock), false);
});

test('removeStaleSocket: 待ち受けているプロセスがいれば例外で、消さない', async () => {
  const sock = path.join(root, 'alive.sock');
  const server = createServer();
  await new Promise<void>((r) => server.listen(sock, r));
  try {
    await assert.rejects(removeStaleSocket(sock), /another daemon is already listening/);
    assert.equal(existsSync(sock), true);
  } finally {
    await new Promise<void>((r) => server.close(() => r()));
  }
});

test('removeStaleSocket: 古いと断定できないエラー (EACCES) では例外で、消さない', async (t) => {
  if (process.getuid?.() === 0) return t.skip('root は権限で拒否されない');
  const sock = path.join(root, 'noperm.sock');
  writeFileSync(sock, '');
  chmodSync(sock, 0o000);
  await assert.rejects(removeStaleSocket(sock), /EACCES/);
  assert.equal(existsSync(sock), true);
});
