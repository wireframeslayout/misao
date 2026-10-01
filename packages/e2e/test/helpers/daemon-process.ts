import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { MisaoClient } from '@misao/sdk';
import { stopChild } from './child.js';
import { nodeCmd } from './node-cmd.js';
import { waitFor } from './wait.js';

const CLI_MAIN = new URL('../../../cli/src/main.ts', import.meta.url).pathname;

export interface DaemonConfig {
  scrollback?: number;
  rings?: { rawBytes?: number; linesBytes?: number; events?: number };
}

export interface DaemonProcess {
  readonly dir: string;
  readonly socket: string;
  readonly configPath: string;
  /** デーモンの PID (起動した子プロセスのもの)。 */
  readonly pid: number;
  /** misao CLI を同じデーモンに向けて動かすための環境変数。 */
  readonly cliEnv: Record<string, string>;
  /** 接続済みのクライアント。stop() で閉じる。 */
  connect(): Promise<MisaoClient>;
  stop(): Promise<void>;
}

/** MISAO_* を除いた環境変数。開発者の実環境のデーモン・設定に触れないための境界。 */
export function cleanEnv(): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined && !key.startsWith('MISAO_')) env[key] = value;
  }
  return env;
}

/** 一時ディレクトリに misao.json を置き、`misao serve` を子プロセスで起動する。 */
export async function startDaemonProcess(config: DaemonConfig = {}): Promise<DaemonProcess> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-e2e-'));
  const socket = path.join(dir, 'misao.sock');
  const configPath = path.join(dir, 'misao.json');
  fs.writeFileSync(configPath, JSON.stringify({ ...config, logLevel: 'warn' }));
  const log = fs.openSync(path.join(dir, 'daemon.log'), 'a');
  const [command, ...args] = nodeCmd(CLI_MAIN, 'serve', '--config', configPath, '--socket', socket, '--data', dir);
  const child: ChildProcess = spawn(command!, args, { env: cleanEnv(), stdio: ['ignore', log, log] });
  fs.closeSync(log);
  const pid = child.pid;
  if (pid === undefined) throw new Error('failed to spawn the daemon');
  const clients: MisaoClient[] = [];
  const stop = async (): Promise<void> => {
    for (const client of clients) client.close();
    await stopChild(child);
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    const earlyExit = new Promise<never>((_, reject) =>
      child.once('exit', (code) => reject(new Error(`daemon exited early (code ${code}): ${fs.readFileSync(path.join(dir, 'daemon.log'), 'utf8')}`))),
    );
    await Promise.race([waitFor(() => fs.existsSync(socket), 'the daemon socket', { timeoutMs: 15_000 }), earlyExit]);
  } catch (error) {
    await stop();
    throw error;
  }
  return {
    dir,
    socket,
    configPath,
    pid,
    cliEnv: { ...cleanEnv(), MISAO_SOCKET: socket },
    async connect() {
      const client = new MisaoClient({ socketPath: socket });
      await client.connect();
      clients.push(client);
      return client;
    },
    stop,
  };
}
