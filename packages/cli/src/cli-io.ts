import * as os from 'node:os';

/** attach / tail / serve が受け取るシグナル。 */
export type CliSignal = 'SIGINT' | 'SIGTERM' | 'SIGHUP' | 'SIGQUIT' | 'SIGWINCH';

export interface CliOutput {
  write(chunk: string | Uint8Array): unknown;
  readonly isTTY?: boolean | undefined;
  readonly columns?: number | undefined;
  readonly rows?: number | undefined;
}

export interface CliInput extends NodeJS.ReadableStream {
  readonly isTTY?: boolean | undefined;
  setRawMode?(mode: boolean): unknown;
}

/** プロセスとの境界。コマンドは process を直接触らず、これを受け取る (テストで差し替える)。 */
export interface CliIo {
  readonly stdout: CliOutput;
  readonly stderr: CliOutput;
  readonly stdin: CliInput;
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly homeDir: string;
  readonly cwd: string;
  readonly pid: number;
  /** ログインシェル。取れない環境では null。 */
  readonly shell: string | null;
  now(): number;
  /** ハンドラを登録し、解除関数を返す。 */
  onSignal(signal: CliSignal, handler: () => void): () => void;
  /** 捕捉されなかった例外。handler で後始末をした後、プロセスを終了コード 1 で終える。 */
  onUncaughtException(handler: (error: Error) => void): () => void;
}

/**
 * ログインシェルを読む。シェルが無い (Windows) か、ユーザー情報を取れない (passwd に無い uid で
 * os.userInfo() が投げる) 環境では null。シェルが要るのは new / 一覧の n だけで、そこで usage エラーにする。
 */
export function readLoginShell(readUserInfo: () => { shell: string | null }): string | null {
  try {
    return readUserInfo().shell;
  } catch {
    return null;
  }
}

export function nodeIo(): CliIo {
  return {
    stdout: process.stdout,
    stderr: process.stderr,
    stdin: process.stdin,
    env: process.env,
    homeDir: os.homedir(),
    cwd: process.cwd(),
    pid: process.pid,
    shell: readLoginShell(() => os.userInfo()),
    now: Date.now,
    onSignal(signal, handler) {
      process.on(signal, handler);
      return () => void process.off(signal, handler);
    },
    onUncaughtException(handler) {
      const listener = (error: Error): void => {
        handler(error);
        process.exit(1);
      };
      process.on('uncaughtException', listener);
      return () => void process.off('uncaughtException', listener);
    },
  };
}
