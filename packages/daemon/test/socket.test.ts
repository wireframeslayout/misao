import assert from 'node:assert/strict';
import { chmodSync, mkdirSync, mkdtempSync, symlinkSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, afterEach, before, test } from 'node:test';
import { ensureSocketDir, listenUnixSocket } from '../src/socket.js';

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

test('listenUnixSocket: ソケットは 0o600、umask は復元される', async () => {
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

test('listenUnixSocket: listen 失敗でも umask を復元して reject', async () => {
  process.umask(0o022);
  const server = createServer();
  await assert.rejects(listenUnixSocket(server, path.join(root, 'nonexistent-dir', 'x.sock')));
  assert.equal(process.umask(), 0o022);
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
