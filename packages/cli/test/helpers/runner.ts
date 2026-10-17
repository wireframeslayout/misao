import { spawn } from 'node:child_process';
import * as pty from 'node-pty';
import { run } from '../../src/run.js';
import { createTestIo } from './io.js';

/** `MISAO_CLI_BIN` (Rust 版の `misao` の絶対パス) を指定すると、コマンドのテストは run() の代わりにそのバイナリを子プロセスで動かす。 */
export const CLI_BIN: string | undefined = process.env['MISAO_CLI_BIN'] || undefined;

export interface RunOptions {
  /** CLI の環境変数 (MISAO_SOCKET など)。バイナリでは PATH / HOME のほかはこれだけを渡す。 */
  env?: Record<string, string>;
  homeDir?: string;
  cwd?: string;
  /** 標準入力に渡す内容。省略すると入力は閉じない。 */
  stdin?: string;
  isTTY?: boolean;
  columns?: number;
  rows?: number;
  /** ログインシェル。バイナリでは passwd のものが使われるので、指定しても効かない (in-process だけ)。 */
  shell?: string | null;
}

export type RunSignal = 'SIGINT' | 'SIGTERM' | 'SIGHUP';

export interface Running {
  out(): string;
  err(): string;
  /** 動いているコマンドへシグナルを送る。 */
  signal(signal: RunSignal): void;
  /** 標準入力 (端末なら入力) へ書く。 */
  write(data: string | Buffer): void;
  /** 端末の大きさを変える (端末で動かしているときだけ。SIGWINCH が届く)。 */
  resize(cols: number, rows: number): void;
  done: Promise<number>;
}

export interface Result {
  code: number;
  out: string;
  err: string;
}

/** コマンドを起動する (in-process か、`MISAO_CLI_BIN` のバイナリ)。 */
export function startMisao(argv: string[], options: RunOptions = {}): Running {
  if (CLI_BIN === undefined) return startInProcess(argv, options);
  return options.isTTY === true ? startOnPty(CLI_BIN, argv, options) : startOnPipes(CLI_BIN, argv, options);
}

export async function runMisao(argv: string[], options: RunOptions = {}): Promise<Result> {
  const running = startMisao(argv, options);
  const code = await running.done;
  return { code, out: running.out(), err: running.err() };
}

function startInProcess(argv: string[], options: RunOptions): Running {
  const io = createTestIo({
    ...(options.env === undefined ? {} : { env: options.env }),
    ...(options.homeDir === undefined ? {} : { homeDir: options.homeDir }),
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    ...(options.isTTY === undefined ? {} : { isTTY: options.isTTY }),
    ...(options.columns === undefined ? {} : { columns: options.columns }),
    ...(options.rows === undefined ? {} : { rows: options.rows }),
    ...(options.shell === undefined ? {} : { shell: options.shell }),
  });
  if (options.stdin !== undefined) io.stdin.end(options.stdin);
  return {
    out: () => io.out(),
    err: () => io.err(),
    signal: (s) => io.emitSignal(s),
    write: (data) => void io.stdin.write(data),
    resize: (cols, rows) => {
      io.setSize(cols, rows);
      io.emitSignal('SIGWINCH');
    },
    done: run(argv, io),
  };
}

function childEnv(options: RunOptions): Record<string, string> {
  const env: Record<string, string> = { PATH: process.env['PATH'] ?? '/usr/bin:/bin', HOME: options.homeDir ?? '/tmp/misao-test-no-home' };
  return { ...env, ...options.env };
}

function startOnPipes(bin: string, argv: string[], options: RunOptions): Running {
  const child = spawn(bin, argv, { env: childEnv(options), cwd: options.cwd ?? '/tmp', stdio: ['pipe', 'pipe', 'pipe'] });
  let out = '';
  let err = '';
  child.stdout.on('data', (c: Buffer) => (out += c.toString('utf8')));
  child.stderr.on('data', (c: Buffer) => (err += c.toString('utf8')));
  child.stdin.on('error', () => undefined);
  if (options.stdin !== undefined) child.stdin.end(options.stdin);
  const done = new Promise<number>((resolve) => child.once('close', (code, signal) => resolve(code ?? (signal === null ? 1 : 128))));
  return {
    out: () => out,
    err: () => err,
    signal: (s) => void child.kill(s),
    write: (data) => void child.stdin.write(data),
    resize: () => undefined,
    done,
  };
}

/** 端末が要るコマンド用。stdout と stderr は 1 本の出力にまとまる (どちらにも同じ全体を返す)。入力の EOF は Ctrl-D。 */
function startOnPty(bin: string, argv: string[], options: RunOptions): Running {
  const term = pty.spawn(bin, argv, {
    name: 'xterm-256color',
    cols: options.columns ?? 100,
    rows: options.rows ?? 30,
    cwd: options.cwd ?? '/tmp',
    env: childEnv(options),
  });
  let all = '';
  term.onData((d) => (all += d));
  const done = new Promise<number>((resolve) => term.onExit((e) => resolve(e.exitCode)));
  if (options.stdin !== undefined) setTimeout(() => term.write(options.stdin === '' ? '\x04' : options.stdin!), 300);
  return {
    out: () => all,
    err: () => all,
    signal: (s) => term.kill(s),
    write: (data) => term.write(data.toString('utf8')),
    resize: (cols, rows) => term.resize(cols, rows),
    done,
  };
}
