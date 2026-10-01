import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from '../../src/daemon.js';
import type { DaemonOptions } from '../../src/daemon.js';
import { RpcClient } from './rpc-client.js';

export async function waitFor(pred: () => boolean | Promise<boolean>, ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!(await pred())) {
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await new Promise((r) => setTimeout(r, 20));
  }
}

/** dir にソケット・pid・persistence.json を置いて起動する。同じ dir で再起動すると状態を引き継ぐ。 */
export async function startDaemon(dir: string, overrides: Partial<DaemonOptions> = {}): Promise<Daemon> {
  const daemon = new Daemon({
    socketPath: path.join(dir, 'misao.sock'),
    pidPath: path.join(dir, 'daemon.pid'),
    statePath: path.join(dir, 'persistence.json'),
    log: () => undefined,
    ...overrides,
  });
  await daemon.start();
  return daemon;
}

export async function withDaemon(fn: (daemon: Daemon, client: RpcClient) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  const daemon = await startDaemon(dir);
  const client = await RpcClient.connect(daemon.socketPath);
  try {
    await fn(daemon, client);
  } finally {
    client.close();
    await daemon.shutdown();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}
