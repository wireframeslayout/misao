import { EventEmitter } from 'node:events';
import * as pty from 'node-pty';
import xterm from '@xterm/headless';
import type { Terminal as TerminalType } from '@xterm/headless';

const { Terminal } = xterm;
type Terminal = TerminalType;
import { nowIso } from '../protocol.js';
import { AnsiLineAssembler } from '../util/ansi.js';
import { SeqRing } from '../util/ring.js';
import { newPaneId } from '../util/ulid.js';
import { flushTerminal, serializeSnapshot, viewportText } from './screen.js';

interface ClientSize {
  cols: number;
  rows: number;
}

export const RAW_RING_BYTES = 1024 * 1024;
export const LINES_RING_BYTES = 64 * 1024;
const SCROLLBACK = 5000;
const KILL_GRACE_MS = 3000;

export interface PaneOpenOptions {
  cmd: string[];
  cwd?: string;
  env?: Record<string, string>;
  cols?: number;
  rows?: number;
  labels?: Record<string, string>;
  socketPath: string;
}

export interface PaneInfo {
  paneId: string;
  pid: number;
  cmd: string[];
  cwd: string;
  labels: Record<string, string>;
  state: 'running' | 'exited';
  exitCode: number | null;
  signal: number | null;
  title: string;
  lastOutputAt: string | null;
  cols: number;
  rows: number;
  clients: string[];
  /** 現在のサイズを決めたクライアント (最後に操作したクライアント)。 */
  sizeOwner: string | null;
}

/** 子プロセス環境: 親の環境から mux 系の変数を除去し、misao 用を足す。 */
export function buildChildEnv(
  base: NodeJS.ProcessEnv,
  paneId: string,
  socketPath: string,
  overrides: Record<string, string> = {},
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(base)) {
    if (v === undefined) continue;
    if (k === 'TMUX' || k === 'TMUX_PANE' || k === 'STY' || k.startsWith('ZELLIJ')) continue;
    env[k] = v;
  }
  env.MISAO_SOCKET = socketPath;
  env.MISAO_PANE_ID = paneId;
  env.TERM = 'xterm-256color';
  env.COLORTERM = 'truecolor';
  return { ...env, ...overrides };
}

/**
 * Pane が emit する:
 *  'output' (seq, data: Buffer, ts)  'line' (seq, text, ts)
 *  'title' (title)  'exit' ({exitCode, signal})
 */
export class Pane extends EventEmitter {
  readonly id = newPaneId();
  readonly rawRing = new SeqRing<Buffer>(RAW_RING_BYTES);
  readonly linesRing = new SeqRing<string>(LINES_RING_BYTES);
  readonly clients = new Set<string>();
  readonly cmd: string[];
  readonly cwd: string;
  readonly labels: Record<string, string>;
  readonly pid: number;
  state: 'running' | 'exited' = 'running';
  exitCode: number | null = null;
  signal: number | null = null;
  title = '';
  lastOutputAt: string | null = null;
  cols: number;
  rows: number;

  private readonly proc: pty.IPty;
  private readonly term: Terminal;
  private readonly assembler = new AnsiLineAssembler();
  private killTimer: NodeJS.Timeout | undefined;
  private readonly clientSizes = new Map<string, ClientSize>();
  sizeOwner: string | null = null;

  constructor(opts: PaneOpenOptions) {
    super();
    this.cmd = opts.cmd;
    this.cwd = opts.cwd ?? process.cwd();
    this.labels = opts.labels ?? {};
    this.cols = opts.cols ?? 80;
    this.rows = opts.rows ?? 24;
    this.term = new Terminal({ cols: this.cols, rows: this.rows, scrollback: SCROLLBACK, allowProposedApi: true });
    this.term.onTitleChange((t) => {
      this.title = t;
      this.emit('title', t);
    });
    this.proc = pty.spawn(opts.cmd[0]!, opts.cmd.slice(1), {
      name: 'xterm-256color',
      cols: this.cols,
      rows: this.rows,
      cwd: this.cwd,
      env: buildChildEnv(process.env, this.id, opts.socketPath, opts.env),
      encoding: null, // バイト列のまま受け取る (UTF-8 境界は自前で扱う)
    });
    this.pid = this.proc.pid;
    this.proc.onData((d: string | Buffer) => this.handleData(typeof d === 'string' ? Buffer.from(d) : d));
    this.proc.onExit(({ exitCode, signal }) => {
      this.state = 'exited';
      const rest = this.assembler.pending;
      if (rest !== '') {
        // 改行なしで終わった最終行も行ストリームに流す
        const ts = nowIso();
        const seq = this.linesRing.push(rest, rest.length + 1, ts);
        this.emit('line', seq, rest, ts);
      }
      this.exitCode = signal ? null : exitCode;
      this.signal = signal || null;
      if (this.killTimer) clearTimeout(this.killTimer);
      this.emit('exit', { exitCode: this.exitCode, signal: this.signal });
    });
  }

  private handleData(data: Buffer): void {
    const ts = nowIso();
    this.lastOutputAt = ts;
    const seq = this.rawRing.push(data, data.length, ts);
    this.term.write(data);
    this.emit('output', seq, data, ts);
    for (const text of this.assembler.push(data)) {
      const lineSeq = this.linesRing.push(text, text.length + 1, ts);
      this.emit('line', lineSeq, text, ts);
    }
  }

  info(): PaneInfo {
    return {
      paneId: this.id,
      pid: this.pid,
      cmd: this.cmd,
      cwd: this.cwd,
      labels: this.labels,
      state: this.state,
      exitCode: this.exitCode,
      signal: this.signal,
      title: this.title,
      lastOutputAt: this.lastOutputAt,
      cols: this.cols,
      rows: this.rows,
      clients: [...this.clients],
      sizeOwner: this.sizeOwner,
    };
  }

  write(data: Buffer): void {
    this.proc.write(data);
  }

  /** clientId のサイズを記録し、そのクライアントをサイズ所有者にして pty / 端末へ適用する。 */
  resize(cols: number, rows: number, clientId: string | null): void {
    if (clientId) this.clientSizes.set(clientId, { cols, rows });
    this.sizeOwner = clientId;
    this.cols = cols;
    this.rows = rows;
    if (this.state === 'running') this.proc.resize(cols, rows);
    this.term.resize(cols, rows);
  }

  /** clientId が所有者でなく、サイズを記録済みなら、そのサイズへ戻す。変更したら true。 */
  claimSize(clientId: string): boolean {
    const size = this.clientSizes.get(clientId);
    if (!size || this.sizeOwner === clientId) return false;
    this.resize(size.cols, size.rows, clientId);
    return true;
  }

  forgetClient(clientId: string): void {
    this.clientSizes.delete(clientId);
    if (this.sizeOwner === clientId) this.sizeOwner = null;
  }

  async screen(): Promise<{ text: string; cursor: { x: number; y: number }; altScreen: boolean; title: string }> {
    await flushTerminal(this.term);
    const buf = this.term.buffer.active;
    return {
      text: viewportText(this.term),
      cursor: { x: buf.cursorX, y: buf.cursorY },
      altScreen: buf.type === 'alternate',
      title: this.title,
    };
  }

  /** 反映済みの状態をシリアライズし、その時点の raw head seq を返す。 */
  snapshot(): Promise<{ data: string; headSeq: number }> {
    const headSeq = this.rawRing.head;
    // xterm は write コールバックの後も、後から積まれたチャンクの解析を同じループで続ける。
    // resolve の続き (マイクロタスク) では遅いので、シリアライズはコールバック内で同期的に行う。
    return new Promise((resolve) =>
      this.term.write('', () => resolve({ data: serializeSnapshot(this.term), headSeq })),
    );
  }

  /** SIGHUP → 3s → SIGKILL。終了で resolve。 */
  close(): Promise<void> {
    if (this.state === 'exited') return Promise.resolve();
    return new Promise((resolve) => {
      this.once('exit', () => resolve());
      this.signalGroup('SIGHUP');
      this.killTimer = setTimeout(() => this.signalGroup('SIGKILL'), KILL_GRACE_MS);
    });
  }

  /** pty の子はセッションリーダーなので、プロセスグループ全体へ送る。失敗したら pid 単体。 */
  private signalGroup(sig: NodeJS.Signals): void {
    try {
      process.kill(-this.pid, sig);
    } catch {
      try {
        process.kill(this.pid, sig);
      } catch {
        // すでに終了
      }
    }
  }

  dispose(): void {
    if (this.killTimer) clearTimeout(this.killTimer);
    this.removeAllListeners();
    this.term.dispose();
  }
}
