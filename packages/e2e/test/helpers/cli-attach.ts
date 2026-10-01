import * as pty from 'node-pty';
import type { DaemonProcess } from './daemon-process.js';
import { REPO_ROOT } from './agents.js';
import { nodeCmd } from './node-cmd.js';
import { waitFor } from './wait.js';

const CLI_MAIN = new URL('../../../cli/src/main.ts', import.meta.url).pathname;

/** 既定の attach 設定 (prefix は Ctrl-^、detach は d)。 */
export const PREFIX_KEY = '\x1e';
export const DETACH_KEY = 'd';

export interface CliAttach {
  output(): string;
  write(data: string): void;
  /** 出力にテキストが現れるまで待つ。 */
  waitForOutput(text: string, timeoutMs?: number): Promise<void>;
  /** CLI の終了を待って終了コードを返す。 */
  exited(timeoutMs?: number): Promise<number>;
  /** 終わっていなければ、起動した CLI プロセスを止める。 */
  kill(): void;
}

/** 疑似端末の中で `misao attach <target>` を動かす。 */
export function startCliAttach(daemon: DaemonProcess, target: string): CliAttach {
  const [command, ...args] = nodeCmd(CLI_MAIN, '--config', daemon.configPath, 'attach', target);
  const term = pty.spawn(command!, args, { name: 'xterm-256color', cols: 100, rows: 30, cwd: REPO_ROOT, env: daemon.cliEnv });
  let out = '';
  let exitCode: number | undefined;
  term.onData((data) => (out += data));
  term.onExit((event) => (exitCode = event.exitCode));
  return {
    output: () => out,
    write: (data) => term.write(data),
    waitForOutput: (text, timeoutMs = 10_000) => waitFor(() => out.includes(text), `CLI output "${text}"`, { timeoutMs }),
    async exited(timeoutMs = 10_000) {
      await waitFor(() => exitCode !== undefined, 'the CLI to exit', { timeoutMs });
      return exitCode!;
    },
    kill() {
      if (exitCode === undefined) term.kill();
    },
  };
}
