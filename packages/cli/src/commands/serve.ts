import * as fs from 'node:fs';
import * as path from 'node:path';
import { Daemon } from '@misao/daemon';
import type { DaemonOptions } from '@misao/daemon';
import { MisaoPathError, resolveSocketPath } from '@misao/sdk';
import type { CliIo } from '../cli-io.js';
import type { ResolvedConfig } from '../config/index.js';
import { CliError } from '../errors.js';
import { writeLine } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals } from './shared.js';

const USAGE = 'misao serve [--socket PATH] [--data DIR]';
const PID_FILE_NAME = 'daemon.pid';
const STATE_FILE_NAME = 'persistence.json';
const DATA_DIR_MODE = 0o700;

export interface ServeOverrides {
  /** 解決済みのソケットパス。省略時は設定のもの。 */
  socket?: string | undefined;
  /** pid と persistence.json の置き場所。省略時はソケットの親ディレクトリ。 */
  dataDir?: string | undefined;
}

/** 設定 (misao.json) と引数から DaemonOptions を組み立てる。設定値はここで Daemon へ渡す。 */
export function buildDaemonOptions(config: ResolvedConfig, overrides: ServeOverrides): DaemonOptions {
  const socketPath = overrides.socket ?? config.socket;
  const dataDir = overrides.dataDir ?? path.dirname(socketPath);
  return {
    socketPath,
    pidPath: path.join(dataDir, PID_FILE_NAME),
    statePath: path.join(dataDir, STATE_FILE_NAME),
    scrollback: config.scrollback,
    rings: { ...config.rings },
    logLevel: config.logLevel,
  };
}

function resolveSocketOverride(io: CliIo, socket: string | undefined): string | undefined {
  if (socket === undefined) return undefined;
  const explicitPath = socket.startsWith('~') ? socket : path.resolve(io.cwd, socket);
  try {
    return resolveSocketPath({ env: {}, explicitPath, homeDir: io.homeDir });
  } catch (error) {
    if (error instanceof MisaoPathError) throw new CliError('usage', `--socket が正しくありません: ${error.message}`);
    throw error;
  }
}

function resolveDataDir(io: CliIo, dir: string | undefined): string | undefined {
  if (dir === undefined) return undefined;
  return dir === '~' || dir.startsWith('~/') ? path.join(io.homeDir, dir.slice(1)) : path.resolve(io.cwd, dir);
}

export const serveCommand: Command = {
  name: 'serve',
  summary: 'デーモンを前面で起動する（SIGTERM / SIGINT で終了）',
  usage: USAGE,
  options: { socket: { type: 'string' }, data: { type: 'string' } },
  async run({ args, io, config }) {
    expectPositionals(args, 0, 0, USAGE);
    const dataDir = resolveDataDir(io, args.string('data'));
    const options = buildDaemonOptions(config, { socket: resolveSocketOverride(io, args.string('socket')), dataDir });
    if (dataDir !== undefined) fs.mkdirSync(dataDir, { recursive: true, mode: DATA_DIR_MODE });
    const daemon = new Daemon({
      ...options,
      log: (msg) => writeLine(io.stderr, `[misao ${new Date(io.now()).toISOString()}] ${msg}`),
    });
    // 起動中に届いたシグナルも取りこぼさないよう、start の前に登録する。
    let stop: () => void = () => undefined;
    const stopped = new Promise<void>((resolve) => (stop = resolve));
    const cleanups = [io.onSignal('SIGINT', stop), io.onSignal('SIGTERM', stop)];
    try {
      await daemon.start();
      await stopped;
      await daemon.shutdown();
      return 0;
    } finally {
      for (const cleanup of cleanups) cleanup();
    }
  },
};
