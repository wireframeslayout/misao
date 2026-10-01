import { PassThrough } from 'node:stream';
import type { CliInput, CliIo, CliSignal } from '../../src/cli-io.js';

export interface TestInput extends PassThrough {
  isTTY?: boolean;
  setRawMode?(mode: boolean): unknown;
}

export interface TestIo extends CliIo {
  readonly stdin: TestInput & CliInput;
  /** stdout / stderr に書かれた内容 (文字列)。 */
  out(): string;
  err(): string;
  /** 登録されているハンドラへシグナルを送る。 */
  emitSignal(signal: CliSignal): void;
  /** 登録されている uncaughtException ハンドラへ送る。 */
  emitUncaught(error: Error): void;
  /** setRawMode(mode) の呼び出し履歴。 */
  readonly rawModes: boolean[];
}

export interface TestIoOptions {
  env?: Record<string, string | undefined>;
  homeDir?: string;
  cwd?: string;
  isTTY?: boolean;
  columns?: number;
  rows?: number;
  shell?: string | null;
}

class Sink extends PassThrough {
  isTTY: boolean | undefined;
  columns: number | undefined;
  rows: number | undefined;
  private text = '';

  constructor(isTTY: boolean | undefined) {
    super();
    this.isTTY = isTTY;
    this.on('data', (c: Buffer) => (this.text += c.toString('utf8')));
  }

  contents(): string {
    return this.text;
  }
}

export function createTestIo(options: TestIoOptions = {}): TestIo {
  const stdout = new Sink(options.isTTY);
  const stderr = new Sink(options.isTTY);
  stdout.columns = options.columns;
  stdout.rows = options.rows;
  const stdin: TestInput = new PassThrough();
  stdin.isTTY = options.isTTY;
  const rawModes: boolean[] = [];
  if (options.isTTY) stdin.setRawMode = (mode: boolean) => rawModes.push(mode);
  const handlers = new Map<CliSignal, Set<() => void>>();
  const uncaught = new Set<(e: Error) => void>();
  return {
    stdout,
    stderr,
    stdin,
    env: options.env ?? {},
    homeDir: options.homeDir ?? '/home/test',
    cwd: options.cwd ?? '/work/test',
    pid: 4242,
    shell: options.shell === undefined ? '/bin/sh' : options.shell,
    now: Date.now,
    onSignal(signal, handler) {
      const set = handlers.get(signal) ?? new Set();
      set.add(handler);
      handlers.set(signal, set);
      return () => void set.delete(handler);
    },
    onUncaughtException(handler) {
      uncaught.add(handler);
      return () => void uncaught.delete(handler);
    },
    out: () => stdout.contents(),
    err: () => stderr.contents(),
    emitSignal: (signal) => handlers.get(signal)?.forEach((h) => h()),
    emitUncaught: (error) => uncaught.forEach((h) => h(error)),
    rawModes,
  };
}
