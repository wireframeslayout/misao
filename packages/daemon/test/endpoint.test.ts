import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import type { Server } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, before, test } from 'node:test';
import {
  assertEndpointPath,
  endpointFiles,
  endpointKind,
  listenEndpoint,
  prepareEndpoint,
  removeStaleEndpoint,
  warnIfDefaultDacl,
} from '../src/endpoint.js';

const PIPE = '\\\\.\\pipe\\misao-u-0123456789abcdef';
let root: string;

before(() => {
  root = mkdtempSync(path.join(tmpdir(), 'misao-endpoint-'));
});
after(() => rmSync(root, { recursive: true, force: true }));

test('endpointKind: win32 だけ pipe', () => {
  assert.equal(endpointKind('win32'), 'pipe');
  for (const p of ['linux', 'darwin', 'freebsd'] as const) assert.equal(endpointKind(p), 'unix');
});

test('assertEndpointPath: OS と接続口の種類が合わなければ理由付きで拒否', () => {
  assertEndpointPath(PIPE, 'win32');
  assertEndpointPath('/tmp/misao.sock', 'linux');
  assert.throws(() => assertEndpointPath('C:\\x\\misao.sock', 'win32'), /must be a named pipe/);
  assert.throws(() => assertEndpointPath(PIPE, 'linux'), /only supported on Windows/);
});

test('endpointFiles: unix はソケットファイル、pipe は実体なし', () => {
  assert.deepEqual(endpointFiles('/tmp/a.sock', 'linux'), ['/tmp/a.sock']);
  assert.deepEqual(endpointFiles(PIPE, 'win32'), []);
});

test('prepareEndpoint(pipe): dataDir を作り、リンクやファイルは拒否（ACL は検証しない）', async () => {
  const dir = path.join(root, 'data', 'nested');
  await prepareEndpoint(PIPE, dir, 'win32');
  assert.ok(statSync(dir).isDirectory());
  const link = path.join(root, 'link');
  symlinkSync(dir, link);
  await assert.rejects(prepareEndpoint(PIPE, link, 'win32'), /symbolic link/);
  const file = path.join(root, 'file');
  writeFileSync(file, '');
  await assert.rejects(prepareEndpoint(PIPE, file, 'win32'));
});

test('prepareEndpoint(unix): 従来どおり親ディレクトリを 700 にする', async () => {
  const dir = path.join(root, 'unix');
  await prepareEndpoint(path.join(dir, 'misao.sock'), dir, 'linux');
  assert.equal(statSync(dir).mode & 0o777, 0o700);
});

test('prepareEndpoint: 種類の合わない接続口は dataDir を作る前に拒否', async () => {
  const dir = path.join(root, 'never');
  await assert.rejects(prepareEndpoint('C:\\x.sock', dir, 'win32'), /named pipe/);
  assert.throws(() => statSync(dir));
});

test('listenEndpoint(pipe): 他のユーザーへ広げる指定をせず、既定 DACL 任せと報告する', async () => {
  const calls: unknown[] = [];
  const fake = {
    once: () => fake,
    off: () => fake,
    listen: (options: unknown, cb: () => void) => {
      calls.push(options);
      cb();
      return fake;
    },
  };
  const protection = await listenEndpoint(fake as unknown as Server, PIPE, 'win32');
  assert.equal(protection, 'default-dacl');
  assert.deepEqual(calls, [{ path: PIPE, readableAll: false, writableAll: false }]);
});

test('listenEndpoint(pipe): listen の error は reject する', async () => {
  const failure = new Error('EADDRINUSE');
  const handlers: Record<string, (e: Error) => void> = {};
  const fake = {
    once: (event: string, h: (e: Error) => void) => {
      handlers[event] = h;
      return fake;
    },
    off: () => fake,
    listen: () => {
      handlers.error?.(failure);
      return fake;
    },
  };
  await assert.rejects(listenEndpoint(fake as unknown as Server, PIPE, 'win32'), failure);
});

test('listenEndpoint(unix): ソケットを 600 で作り posix-mode と報告する', async () => {
  const dir = path.join(root, 'listen');
  const sock = path.join(dir, 'misao.sock');
  await prepareEndpoint(sock, dir, 'linux');
  const server = createServer();
  try {
    assert.equal(await listenEndpoint(server, sock, 'linux'), 'posix-mode');
    assert.equal(statSync(sock).mode & 0o777, 0o600);
  } finally {
    server.close();
  }
});

test('removeStaleEndpoint(unix): 従来どおり stale なソケットファイルを消す', async () => {
  const sock = path.join(root, 'stale.sock');
  writeFileSync(sock, '');
  assert.equal(await removeStaleEndpoint(sock, 'linux'), true);
  assert.equal(await removeStaleEndpoint(sock, 'linux'), false);
});

test('removeStaleEndpoint(pipe): パイプが無ければ (ENOENT) false で、何も消さない', async () => {
  assert.equal(await removeStaleEndpoint(PIPE, 'win32'), false);
});

test('removeStaleEndpoint(pipe): 待ち受けがいれば例外 (パイプ名の代わりに稼働中の Unix ソケットで代用して分岐を確かめる)', async () => {
  const sock = path.join(root, 'alive-pipe.sock');
  const server = createServer();
  await new Promise<void>((r) => server.listen(sock, r));
  try {
    await assert.rejects(removeStaleEndpoint(sock, 'win32'), /another daemon is already listening/);
  } finally {
    await new Promise<void>((r) => server.close(() => r()));
  }
});

test('warnIfDefaultDacl: default-dacl のときだけ警告する', () => {
  const messages: string[] = [];
  assert.equal(messages.length, 0);
  assert.deepEqual(messages, []);
  warnIfDefaultDacl('default-dacl', (m) => messages.push(m));
  assert.equal(messages.length, 1);
  assert.match(messages[0]!, /default DACL/);
});
