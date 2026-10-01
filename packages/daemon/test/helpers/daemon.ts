import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from '../../src/daemon.js';
import { RpcClient } from './rpc-client.js';

export function waitFor(pred: () => boolean, ms = 5000): Promise<void> {
  return new Promise((resolve, reject) => {
    const t0 = Date.now();
    const tick = (): void => {
      if (pred()) resolve();
      else if (Date.now() - t0 > ms) reject(new Error('timeout'));
      else setTimeout(tick, 20);
    };
    tick();
  });
}

export async function withDaemon(fn: (daemon: Daemon, client: RpcClient) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-it-'));
  const daemon = new Daemon({
    socketPath: path.join(dir, 'misao.sock'),
    pidPath: path.join(dir, 'daemon.pid'),
    log: () => undefined,
  });
  await daemon.start();
  const client = await RpcClient.connect(daemon.socketPath);
  try {
    await fn(daemon, client);
  } finally {
    client.close();
    await daemon.shutdown();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}
