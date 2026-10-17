import { spawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { Daemon } from '@misao/daemon';
import type { DaemonOptions } from '@misao/daemon';
import { MisaoClient } from '@misao/sdk';

export interface TestDaemon {
  readonly dir: string;
  /** デーモンの pid (in-process なら process.pid、MISAO_DAEMON_BIN なら起動した子プロセス)。 */
  readonly pid: number;
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

/**
 * tmpdir に実デーモンを起動する。ペインは stop() の shutdown で閉じる。
 * `MISAO_DAEMON_BIN` (Rust 版の `misao` の絶対パス) を指定すると、in-process の Node 版の代わりにそのバイナリを
 * 子プロセスとして起動する (overrides のうち scrollback / rings / logLevel だけを misao.json で渡せる)。
 */
export async function startTestDaemon(overrides: Partial<DaemonOptions> = {}): Promise<TestDaemon> {
  const bin = process.env['MISAO_DAEMON_BIN'];
  return bin === undefined || bin === '' ? startInProcessDaemon(overrides) : startBinaryDaemon(bin, overrides);
}

async function startBinaryDaemon(bin: string, overrides: Partial<DaemonOptions>): Promise<TestDaemon> {
  const unsupported = Object.keys(overrides).filter((k) => !['scrollback', 'rings', 'logLevel'].includes(k));
  if (unsupported.length > 0) throw new Error(`MISAO_DAEMON_BIN cannot honor these overrides: ${unsupported.join(', ')}`);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-cli-'));
  const socketPath = path.join(dir, 'misao.sock');
  const configPath = path.join(dir, 'misao.json');
  fs.writeFileSync(configPath, JSON.stringify({ logLevel: 'warn', ...overrides }));
  const log = fs.openSync(path.join(dir, 'daemon.log'), 'a');
  const child = spawn(bin, ['serve', '--config', configPath, '--socket', socketPath, '--data', dir], {
    env: { PATH: process.env['PATH'] ?? '/usr/bin:/bin', HOME: dir },
    stdio: ['ignore', log, log],
  });
  fs.closeSync(log);
  const exited = new Promise<void>((resolve) => child.once('close', () => resolve()));
  const stopChild = async (): Promise<void> => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    await exited;
  };
  try {
    await waitFor(() => fs.existsSync(socketPath), 15_000);
  } catch (error) {
    await stopChild();
    throw error;
  }
  const client = new MisaoClient({ socketPath });
  await client.connect();
  return {
    dir,
    pid: child.pid!,
    socketPath,
    env: { MISAO_SOCKET: socketPath },
    client,
    async stop() {
      client.close();
      await stopChild();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

async function startInProcessDaemon(overrides: Partial<DaemonOptions>): Promise<TestDaemon> {
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
    pid: process.pid,
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
