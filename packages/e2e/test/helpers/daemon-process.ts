import { execFile, spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { MisaoClient } from '@misao/sdk';
import { stopChild } from './child.js';
import { nodeCmd } from './node-cmd.js';
import { IS_WINDOWS, filePath } from './platform.js';
import { sleep, waitFor } from './wait.js';

const CLI_MAIN = filePath('../../../cli/src/main.ts', import.meta.url);

/** どちらの実装のデーモンか。省略時は `MISAO_DAEMON_BIN` があれば Rust 版、無ければ Node 版。 */
export type DaemonImpl = 'node' | 'rust';

export interface DaemonOptions {
  /** 既存のディレクトリ（ソケット・pid・保存した状態の置き場）で起動する。stop() でもディレクトリは消さない（再起動の確認用）。 */
  dir?: string;
  impl?: DaemonImpl;
}

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

/**
 * デーモンを起動する argv。`MISAO_DAEMON_BIN` を指定すると、Node 版の代わりにその実行ファイル
 * （Rust 版の `target/debug/misao` など）で `misao serve` を動かす。未設定なら Node 版。
 * `MISAO_*` は cleanEnv が子へ渡さないので、ここでだけ読む。
 */
function daemonCmd(configPath: string, socket: string, dir: string, impl: DaemonImpl | undefined): string[] {
  const serveArgs = ['serve', '--config', configPath, '--socket', socket, '--data', dir];
  const bin = process.env['MISAO_DAEMON_BIN'];
  const hasBin = bin !== undefined && bin !== '';
  if (impl === 'rust' && !hasBin) throw new Error('MISAO_DAEMON_BIN is required to start the Rust daemon');
  return (impl ?? (hasBin ? 'rust' : 'node')) === 'rust' ? [bin!, ...serveArgs] : nodeCmd(CLI_MAIN, ...serveArgs);
}

/** 接続口の名前。Unix はディレクトリの中のソケット、Windows は一意な名前付きパイプ。 */
function endpointName(dir: string): string {
  return IS_WINDOWS ? `\\\\.\\pipe\\misao-e2e-${process.pid}-${path.basename(dir)}` : path.join(dir, 'misao.sock');
}

/** 接続口に接続できるか（Windows のパイプにはファイルとしての存在確認が使えない）。 */
function canConnect(socket: string): Promise<boolean> {
  return new Promise((resolve) => {
    const connection = net.connect(socket);
    connection.once('connect', () => {
      connection.destroy();
      resolve(true);
    });
    connection.once('error', () => resolve(false));
  });
}

/**
 * デーモンを止める。Unix は SIGTERM。Windows には届けられるシグナルが無いので、`misao stop`（デーモンのバイナリ自身）で止める
 * （ペインを閉じ、状態を保存して終わる）。終わらなければ強制終了する。
 */
async function stopDaemon(child: ChildProcess, socket: string, env: Record<string, string>): Promise<void> {
  const bin = process.env['MISAO_DAEMON_BIN'];
  if (IS_WINDOWS && bin !== undefined && bin !== '' && child.exitCode === null) {
    await new Promise<void>((resolve) => execFile(bin, ['stop'], { env: { ...env, MISAO_SOCKET: socket }, timeout: 20_000 }, () => resolve()));
    await Promise.race([new Promise<void>((resolve) => child.once('exit', () => resolve())), sleep(5000)]);
  }
  await stopChild(child);
}

/** 一時ディレクトリに misao.json を置き、`misao serve` を子プロセスで起動する。 */
export async function startDaemonProcess(config: DaemonConfig = {}, options: DaemonOptions = {}): Promise<DaemonProcess> {
  const dir = options.dir ?? fs.mkdtempSync(path.join(os.tmpdir(), 'misao-e2e-'));
  const socket = endpointName(dir);
  const configPath = path.join(dir, 'misao.json');
  fs.writeFileSync(configPath, JSON.stringify({ ...config, logLevel: 'warn' }));
  const log = fs.openSync(path.join(dir, 'daemon.log'), 'a');
  const [command, ...args] = daemonCmd(configPath, socket, dir, options.impl);
  const child: ChildProcess = spawn(command!, args, { env: cleanEnv(), stdio: ['ignore', log, log] });
  fs.closeSync(log);
  const pid = child.pid;
  if (pid === undefined) throw new Error('failed to spawn the daemon');
  const clients: MisaoClient[] = [];
  const stop = async (): Promise<void> => {
    for (const client of clients) client.close();
    await stopDaemon(child, socket, cleanEnv());
    if (options.dir === undefined) fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    const earlyExit = new Promise<never>((_, reject) =>
      child.once('exit', (code) => reject(new Error(`daemon exited early (code ${code}): ${fs.readFileSync(path.join(dir, 'daemon.log'), 'utf8')}`))),
    );
    const isUp = IS_WINDOWS ? () => canConnect(socket) : () => fs.existsSync(socket);
    await Promise.race([waitFor(isUp, 'the daemon socket', { timeoutMs: 15_000 }), earlyExit]);
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
