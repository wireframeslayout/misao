// 検証スクリプト共通: 自前デーモンの起動/停止 (PID のみ)、クライアント視点の画面再構成。
import { spawn, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import xterm from '@xterm/headless';
import { MisaoClient } from '../src/client/MisaoClient.js';
import { viewportText } from '../src/daemon/screen.js';
import type { PaneInfo } from '../src/daemon/Pane.js';

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

export interface DaemonHandle {
  dir: string;
  pid: number;
  socket: string;
  stop(): Promise<void>;
}

/** .run/<name> で daemon を起動する。終了時は自分の PID にだけ SIGTERM。 */
export async function startDaemon(name: string): Promise<DaemonHandle> {
  const dir = path.join(ROOT, '.run', name);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  const log = fs.openSync(path.join(dir, 'daemon.log'), 'a');
  const child: ChildProcess = spawn(process.execPath, [path.join(ROOT, 'dist/cli.js'), 'serve', '--dir', dir], {
    env: { ...process.env, MISAO_SPIKE_TAG: `verify-${name}` },
    stdio: ['ignore', log, log],
  });
  const exited = new Promise<void>((r) => child.once('exit', () => r()));
  const socket = path.join(dir, 'misao.sock');
  for (let i = 0; i < 100 && !fs.existsSync(socket); i++) await sleep(50);
  if (!fs.existsSync(socket)) throw new Error('daemon did not start');
  return {
    dir,
    pid: child.pid!,
    socket,
    async stop() {
      child.kill('SIGTERM');
      await Promise.race([exited, sleep(8000)]);
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    },
  };
}

export async function listPanes(c: MisaoClient): Promise<PaneInfo[]> {
  return c.request<PaneInfo[]>('pane.list');
}

export async function paneInfo(c: MisaoClient, paneId: string): Promise<PaneInfo> {
  const p = (await listPanes(c)).find((x) => x.paneId === paneId);
  if (!p) throw new Error(`pane gone: ${paneId}`);
  return p;
}

/** lastOutputAt が quietMs 動かなくなるまで待つ。 */
export async function waitQuiet(c: MisaoClient, paneId: string, quietMs = 800, maxMs = 60000): Promise<void> {
  const t0 = Date.now();
  let last = '';
  let since = Date.now();
  while (Date.now() - t0 < maxMs) {
    const cur = (await paneInfo(c, paneId)).lastOutputAt ?? '';
    if (cur !== last) {
      last = cur;
      since = Date.now();
    } else if (Date.now() - since >= quietMs) return;
    await sleep(100);
  }
}

export async function waitFor(pred: () => boolean | Promise<boolean>, ms = 30000, step = 100): Promise<boolean> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await pred()) return true;
    await sleep(step);
  }
  return false;
}

/** 1 クライアントの視点: 受信した pane.output を自サイズの headless 端末に流して画面を再構成する。 */
export class ClientView {
  readonly term: InstanceType<typeof xterm.Terminal>;
  bytes = 0;
  private chain: Promise<void> = Promise.resolve();

  private constructor(
    readonly client: MisaoClient,
    readonly clientId: string,
    readonly paneId: string,
    public cols: number,
    public rows: number,
  ) {
    this.term = new xterm.Terminal({ cols, rows, scrollback: 1000, allowProposedApi: true });
    client.onNotification((n) => {
      if (n.method !== 'pane.output' || n.params.paneId !== paneId) return;
      const b = Buffer.from(n.params.dataB64 as string, 'base64');
      this.bytes += b.length;
      this.chain = this.chain.then(() => new Promise<void>((r) => this.term.write(b, r)));
    });
  }

  static async attach(
    socket: string,
    paneId: string,
    clientId: string,
    replay: 'raw' | 'snapshot' | 'none',
    size: { cols: number; rows: number },
    opts: { announceSize?: boolean } = {},
  ): Promise<ClientView> {
    const client = await MisaoClient.connect(socket);
    const v = new ClientView(client, clientId, paneId, size.cols, size.rows);
    const params: Record<string, unknown> = { paneId, clientId, replay };
    if (opts.announceSize) Object.assign(params, size);
    await client.request('pane.attach', params);
    return v;
  }

  resizeView(cols: number, rows: number): void {
    this.cols = cols;
    this.rows = rows;
    this.term.resize(cols, rows);
  }

  async text(): Promise<string> {
    await this.chain;
    await new Promise<void>((r) => this.term.write('', r));
    return viewportText(this.term);
  }

  async write(data: string): Promise<void> {
    await this.client.request('pane.write', { paneId: this.paneId, data, source: 'terminal', clientId: this.clientId });
  }

  close(): void {
    this.client.close();
  }
}

export interface Comparison {
  rows: number;
  matchedRows: number;
  matchRate: number;
  mismatches: Array<{ row: number; view: string; daemon: string }>;
}

/** 行ごとに末尾空白を無視して比較する。cols/rows は小さい側に合わせる。 */
export function compareScreens(viewText: string, daemonText: string, maxCols?: number, maxRows?: number): Comparison {
  const norm = (s: string) => (maxCols ? s.slice(0, maxCols) : s).replace(/\s+$/, '');
  let a = viewText.split('\n');
  let b = daemonText.split('\n');
  const n = Math.min(maxRows ?? Math.max(a.length, b.length), Math.max(a.length, b.length));
  a = a.slice(0, n);
  b = b.slice(0, n);
  const mismatches: Comparison['mismatches'] = [];
  let matched = 0;
  for (let i = 0; i < n; i++) {
    const x = norm(a[i] ?? '');
    const y = norm(b[i] ?? '');
    if (x === y) matched++;
    else if (mismatches.length < 10) mismatches.push({ row: i, view: x, daemon: y });
  }
  return { rows: n, matchedRows: matched, matchRate: n ? matched / n : 1, mismatches };
}

export function writeResult(name: string, data: unknown): void {
  const p = path.join(ROOT, 'verify/results', `${name}.json`);
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, JSON.stringify(data, null, 2) + '\n');
  process.stderr.write(`[verify] wrote ${p}\n`);
}

/** claude / codex を pane で動かすとき、親の CLAUDE* 系変数を引き継がない cmd 接頭辞。 */
export function cleanEnvPrefix(): string[] {
  const names = Object.keys(process.env).filter((k) => /^(CLAUDE|CODEX|ANTHROPIC_)/.test(k) && k !== 'ANTHROPIC_API_KEY');
  return names.length ? ['env', ...names.flatMap((n) => ['-u', n])] : [];
}

/** tmux 系変数が pane に渡っていないことを画面から確認するためのヘルパは不要 (daemon が除去する)。 */
export async function open(c: MisaoClient, cmd: string[], extra: Record<string, unknown> = {}): Promise<string> {
  const r = await c.request<{ paneId: string }>('pane.open', { cmd, cols: 100, rows: 30, ...extra });
  return r.paneId;
}

export async function closeAll(c: MisaoClient): Promise<void> {
  for (const p of await listPanes(c)) await c.request('pane.close', { paneId: p.paneId }).catch(() => undefined);
}

/** 監視クライアント: 出力バイト総数と、pane.resized イベントを記録する。 */
export async function monitor(socket: string): Promise<{
  client: MisaoClient;
  bytes: () => number;
  resized: Array<Record<string, unknown>>;
  events: Array<Record<string, unknown>>;
}> {
  const client = await MisaoClient.connect(socket);
  let bytes = 0;
  const resized: Array<Record<string, unknown>> = [];
  const events: Array<Record<string, unknown>> = [];
  client.onNotification((n) => {
    if (n.method === 'pane.output') bytes += Buffer.from(n.params.dataB64 as string, 'base64').length;
    if (n.method === 'event') {
      events.push({ seq: n.params.seq, type: n.params.type, data: n.params.data });
      if (n.params.type === 'pane.resized') resized.push(n.params.data as Record<string, unknown>);
    }
  });
  await client.request('events.subscribe', {});
  return { client, bytes: () => bytes, resized, events };
}

/** Claude Code の「フォルダを信頼するか」ダイアログを、「Yes」を選んで通過する。出なければ何もしない。 */
export async function passTrustDialog(c: MisaoClient, paneId: string, maxMs = 20000): Promise<string | null> {
  const t0 = Date.now();
  let seen: string | null = null;
  while (Date.now() - t0 < maxMs) {
    const text = (await c.request<{ text: string }>('pane.screen', { paneId })).text;
    if (/trust this folder/i.test(text)) {
      seen = text.split('\n').filter(Boolean).slice(0, 8).join(' | ');
      const sel = text.split('\n').find((l) => l.includes('❯')) ?? '';
      const keys = /Yes/i.test(sel) ? '\r' : '\x1b[B';
      await c.request('pane.write', { paneId, data: keys, source: 'hub' });
      await sleep(500);
      if (keys !== '\r') await c.request('pane.write', { paneId, data: '\r', source: 'hub' });
      await sleep(1500);
      continue;
    }
    if (seen || text.trim().length > 40) return seen;
    await sleep(500);
  }
  return seen;
}
