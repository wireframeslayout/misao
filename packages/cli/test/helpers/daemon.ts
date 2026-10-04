import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from '@misao/daemon';
import type { DaemonOptions } from '@misao/daemon';
import { MisaoClient } from '@misao/sdk';

export interface TestDaemon {
  readonly dir: string;
  readonly socketPath: string;
  /** CLI の io.env に渡す。 */
  readonly env: Record<string, string>;
  /** 直接 RPC を呼ぶためのクライアント。 */
  readonly client: MisaoClient;
  stop(): Promise<void>;
}

export async function waitFor(pred: () => boolean | Promise<boolean>, ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!(await pred())) {
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await new Promise((r) => setTimeout(r, 20));
  }
}

/** tmpdir に実デーモンを起動する。ペインは stop() の shutdown で閉じる。 */
export async function startTestDaemon(overrides: Partial<DaemonOptions> = {}): Promise<TestDaemon> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-cli-'));
  const socketPath = path.join(dir, 'misao.sock');
  const daemon = new Daemon({
    version: '0.0.0-test',
    socketPath,
    pidPath: path.join(dir, 'daemon.pid'),
    statePath: path.join(dir, 'persistence.json'),
    log: () => undefined,
    ...overrides,
  });
  await daemon.start();
  const client = new MisaoClient({ socketPath });
  await client.connect();
  return {
    dir,
    socketPath,
    env: { MISAO_SOCKET: socketPath },
    client,
    async stop() {
      client.close();
      await daemon.shutdown();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** 短命でないコマンドでペインを作る。 */
export async function openTestPane(
  client: MisaoClient,
  cmd: string[],
  extra: { labels?: Record<string, string>; cwd?: string } = {},
): Promise<string> {
  const { paneId } = await client.request('pane.open', { cmd, ...extra });
  return paneId;
}
