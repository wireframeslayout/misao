import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';
import { MisaoClient } from '@misao/sdk';
import { buildDaemonOptions } from '../../src/commands/serve.js';
import type { ResolvedConfig } from '../../src/config/index.js';
import { run } from '../../src/run.js';
import { createTestIo } from '../helpers/io.js';
import type { TestIo } from '../helpers/io.js';
import { waitFor } from '../helpers/daemon.js';

const CONFIG: ResolvedConfig = {
  keys: { prefix: 0x1e, detach: 0x64, next: 0x6e, prev: 0x70, list: 0x6c },
  scrollback: 7,
  rings: { rawBytes: 111, linesBytes: 222, events: 3 },
  logLevel: 'warn',
  socket: '/run/misao/misao.sock',
};

test('buildDaemonOptions: 設定の scrollback / rings / logLevel を DaemonOptions に渡す', () => {
  assert.deepEqual(buildDaemonOptions(CONFIG, {}), {
    socketPath: '/run/misao/misao.sock',
    pidPath: '/run/misao/daemon.pid',
    statePath: '/run/misao/persistence.json',
    scrollback: 7,
    rings: { rawBytes: 111, linesBytes: 222, events: 3 },
    logLevel: 'warn',
  });
});

test('buildDaemonOptions: --socket はソケットだけでなくデータの既定も動かし、--data は pid / persistence だけを動かす', () => {
  const moved = buildDaemonOptions(CONFIG, { socket: '/tmp/x/a.sock' });
  assert.deepEqual([moved.socketPath, moved.pidPath, moved.statePath], ['/tmp/x/a.sock', '/tmp/x/daemon.pid', '/tmp/x/persistence.json']);
  const data = buildDaemonOptions(CONFIG, { socket: '/tmp/x/a.sock', dataDir: '/var/lib/misao' });
  assert.deepEqual([data.socketPath, data.pidPath, data.statePath], ['/tmp/x/a.sock', '/var/lib/misao/daemon.pid', '/var/lib/misao/persistence.json']);
});

interface Serving {
  io: TestIo;
  socket: string;
  dataDir: string;
  exit: Promise<number>;
}

/** misao.json を置いて `misao serve` を起動し、つながるまで待つ。 */
async function startServe(dir: string, config: Record<string, unknown>): Promise<Serving> {
  const socket = path.join(dir, 'run', 'misao.sock');
  const dataDir = path.join(dir, 'data');
  const configPath = path.join(dir, 'misao.json');
  fs.writeFileSync(configPath, JSON.stringify(config));
  const io = createTestIo({ homeDir: dir, cwd: dir });
  const exit = run(['serve', '--config', configPath, '--socket', socket, '--data', dataDir], io);
  await waitFor(() => fs.existsSync(socket));
  return { io, socket, dataDir, exit };
}

test('serve: misao.json の上限と logLevel が実際のデーモンまで届き、SIGINT で shutdown して 0', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-serve-'));
  try {
    const s = await startServe(dir, { rings: { events: 2, linesBytes: 30 }, logLevel: 'error' });
    const client = new MisaoClient({ socketPath: s.socket });
    await client.connect();
    const { paneId } = await client.request('pane.open', {
      cmd: ['sh', '-c', 'for i in 1 2 3 4 5 6; do echo "line-number-$i"; done; sleep 60'],
    });
    await waitFor(async () => (await client.request('pane.screen', { paneId })).text.includes('line-number-6'));
    assert.equal((await client.request('events.subscribe', { since: 0 })).gap, true, 'rings.events=2');
    assert.equal((await client.request('pane.subscribe_lines', { paneId, since: 0 })).gap, true, 'rings.linesBytes=30');
    assert.equal(s.io.err().includes('listening'), false, 'logLevel=error');
    assert.ok(fs.existsSync(path.join(s.dataDir, 'daemon.pid')), '--data に pid を置く');
    assert.ok(fs.existsSync(path.join(s.dataDir, 'persistence.json')), '--data に persistence.json を置く');
    client.close();

    s.io.emitSignal('SIGINT');
    assert.equal(await s.exit, 0);
    assert.equal(fs.existsSync(s.socket), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('serve: 既定の logLevel (info) では listening を stderr に出し、SIGTERM でも 0', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-serve-'));
  try {
    const s = await startServe(dir, {});
    await waitFor(() => s.io.err().includes('listening on'));
    s.io.emitSignal('SIGTERM');
    assert.equal(await s.exit, 0);
    assert.match(s.io.err(), /shutting down/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('serve: 長すぎる --socket は使い方の誤り (2)', async () => {
  const io = createTestIo({ homeDir: '/tmp/h', cwd: '/tmp' });
  assert.equal(await run(['serve', '--socket', `/tmp/${'x'.repeat(200)}/misao.sock`], io), 2);
  assert.match(io.err(), /--socket/);
});
