import type { CliIo } from '../cli-io.js';
import { CliError } from '../errors.js';
import { writeLine } from '../output.js';

/** alt screen / マウス / bracketed paste / アプリカーソル / SGR / カーソル表示を既定へ戻す。 */
export const TTY_RESET =
  '\x1b[0m\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b[?25h\x1b[?9l\x1b[?1004l\x1b>';

/** attach と一覧で受けて終了する (終了コード 1) シグナル。 */
export const TERMINATION_SIGNALS = ['SIGTERM', 'SIGHUP', 'SIGINT', 'SIGQUIT'] as const;

/**
 * 端末を raw mode にして、どう終わっても (正常終了 / シグナル / 例外) 元に戻す。
 * 戻す処理は restore() で 1 回だけ行う。
 */
export class TtyGuard {
  private cleanups: Array<() => void> = [];
  private isActive = false;

  constructor(private readonly io: CliIo) {}

  /** raw mode に入る。SIGTERM / SIGHUP / SIGINT / SIGQUIT では端末を戻してから onSignal を呼ぶ。 */
  enter(onSignal: () => void): void {
    const { stdin } = this.io;
    if (!stdin.isTTY || stdin.setRawMode === undefined) {
      throw new CliError('usage', 'attach には端末（TTY）が必要です');
    }
    stdin.setRawMode(true);
    stdin.resume();
    this.isActive = true;
    const interrupt = (): void => {
      this.restore();
      onSignal();
    };
    // raw mode 中の Ctrl-C はバイトとして pane に送られる。ここで受けるのは kill -INT など外からのシグナル。
    this.cleanups = [
      ...TERMINATION_SIGNALS.map((signal) => this.io.onSignal(signal, interrupt)),
      this.io.onUncaughtException((error) => {
        this.restore();
        writeLine(this.io.stderr, `[misao] 予期しないエラー: ${error.message}`);
      }),
    ];
  }

  /** raw mode を戻して端末を既定の状態にする。何度呼んでも 1 回だけ実行する。 */
  restore(): void {
    if (!this.isActive) return;
    this.isActive = false;
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
    this.io.stdin.setRawMode?.(false);
    this.io.stdout.write(TTY_RESET);
    this.io.stdin.pause();
  }
}

export interface TerminalSize {
  cols: number;
  rows: number;
}

/** stdin / stdout が端末でなければ usage エラー。 */
export function requireTerminal(io: CliIo): void {
  if (!io.stdin.isTTY || !io.stdout.isTTY) throw new CliError('usage', 'attach には端末（TTY）が必要です');
}

/** 端末のサイズ。端末が 0 や不明を報告したら undefined（ペインのサイズは変えない）。 */
export function terminalSize(io: CliIo): TerminalSize | undefined {
  const { columns, rows } = io.stdout;
  if (columns === undefined || rows === undefined || columns <= 0 || rows <= 0) return undefined;
  return { cols: columns, rows };
}
