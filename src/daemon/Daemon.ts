import { EventEmitter } from 'node:events';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as path from 'node:path';
import {
  ErrorCode,
  LineSplitter,
  PROTOCOL_VERSION,
  RpcFailure,
  encodeMessage,
  isRequest,
  nowIso,
  type RpcMessage,
  type RpcNotification,
  type RpcRequest,
} from '../protocol.js';
import { SeqRing } from '../util/ring.js';
import { ulid } from '../util/ulid.js';
import { Pane } from './Pane.js';

const EVENT_RING_CAP = 1000;
/** これ以上送信キューが溜まった接続 (遅い consumer) は切る。 */
const MAX_WRITABLE_BYTES = 16 * 1024 * 1024;

type Params = Record<string, unknown>;

interface Ctx {
  conn: Connection;
  afterReply(fn: () => void): void;
}
type Handler = (params: Params, ctx: Ctx) => Promise<unknown> | unknown;

interface DaemonEvent {
  type: string;
  data: Record<string, unknown>;
}

class Connection {
  private readonly splitter = new LineSplitter();
  private readonly closers: Array<() => void> = [];
  /** paneId → clientId */
  readonly attachments = new Map<string, { clientId: string; off: () => void }>();
  closed = false;

  constructor(
    readonly socket: net.Socket,
    onMessage: (conn: Connection, msg: RpcMessage | undefined, parseError?: string) => void,
    onClose: (conn: Connection) => void,
  ) {
    socket.on('data', (chunk) => {
      for (const line of this.splitter.push(chunk)) {
        try {
          onMessage(this, JSON.parse(line) as RpcMessage);
        } catch (e) {
          onMessage(this, undefined, (e as Error).message);
        }
      }
    });
    socket.on('error', () => socket.destroy());
    socket.on('close', () => {
      this.closed = true;
      for (const fn of this.closers.splice(0)) fn();
      onClose(this);
    });
  }

  onClose(fn: () => void): void {
    this.closers.push(fn);
  }

  send(msg: RpcMessage): void {
    if (this.closed || this.socket.destroyed) return;
    if (this.socket.writableLength > MAX_WRITABLE_BYTES) {
      this.socket.destroy();
      return;
    }
    this.socket.write(encodeMessage(msg));
  }

  notify(method: string, seq: number, ts: string, params: Params): void {
    this.send({ jsonrpc: '2.0', method, params: { seq, ts, ...params } } satisfies RpcNotification);
  }
}

export interface DaemonOptions {
  dir: string;
  log?: (msg: string) => void;
}

export class Daemon {
  readonly socketPath: string;
  private readonly pidPath: string;
  private readonly log: (msg: string) => void;
  private readonly panes = new Map<string, Pane>();
  private readonly eventRing = new SeqRing<DaemonEvent>(EVENT_RING_CAP);
  private readonly bus = new EventEmitter();
  private readonly conns = new Set<Connection>();
  private server: net.Server | undefined;
  private readonly startedAt = Date.now();
  /** 起動ごとに変わる ID。seq の巻き戻り (デーモン再起動) をクライアントが検知するためのもの。 */
  private readonly epoch = ulid();
  private readonly handlers: Record<string, Handler>;

  constructor(private readonly opts: DaemonOptions) {
    this.socketPath = path.join(opts.dir, 'misao.sock');
    this.pidPath = path.join(opts.dir, 'daemon.pid');
    this.log = opts.log ?? ((m) => process.stderr.write(`[misao ${nowIso()}] ${m}\n`));
    this.handlers = {
      'server.info': () => this.serverInfo(),
      'pane.open': (p) => this.paneOpen(p),
      'pane.list': () => [...this.panes.values()].map((pane) => pane.info()),
      'pane.write': (p) => this.paneWrite(p),
      'pane.resize': (p) => this.paneResize(p),
      'pane.screen': (p) => this.pane(p).screen(),
      'pane.attach': (p, ctx) => this.paneAttach(p, ctx),
      'pane.detach': (p, ctx) => this.paneDetach(p, ctx),
      'pane.subscribe_lines': (p, ctx) => this.subscribeLines(p, ctx),
      'events.subscribe': (p, ctx) => this.subscribeEvents(p, ctx),
      'pane.close': (p) => this.paneClose(p),
    };
  }

  async start(): Promise<void> {
    fs.mkdirSync(this.opts.dir, { recursive: true, mode: 0o700 });
    await this.removeStaleSocket();
    this.server = net.createServer((socket) => this.accept(socket));
    await new Promise<void>((resolve, reject) => {
      this.server!.once('error', reject);
      this.server!.listen(this.socketPath, () => resolve());
    });
    fs.chmodSync(this.socketPath, 0o600);
    fs.writeFileSync(this.pidPath, String(process.pid));
    this.emitEvent('daemon.started', { pid: process.pid, version: PROTOCOL_VERSION });
    this.log(`listening on ${this.socketPath} (pid ${process.pid})`);
  }

  /** 応答するデーモンがいなければ古い socket ファイルを消す。いれば例外。 */
  private async removeStaleSocket(): Promise<void> {
    if (!fs.existsSync(this.socketPath)) return;
    const alive = await new Promise<boolean>((resolve) => {
      const s = net.connect(this.socketPath);
      s.once('connect', () => {
        s.destroy();
        resolve(true);
      });
      s.once('error', () => resolve(false));
    });
    if (alive) throw new Error(`another daemon is already listening on ${this.socketPath}`);
    fs.unlinkSync(this.socketPath);
    this.log('removed stale socket');
  }

  async shutdown(): Promise<void> {
    this.log('shutting down');
    // close() は接続が全部閉じるまで完了しないので、先に接続を切ってから待つ。
    const closed = new Promise<void>((resolve) => (this.server ? this.server.close(() => resolve()) : resolve()));
    for (const conn of this.conns) conn.socket.destroy();
    await closed;
    await Promise.all([...this.panes.values()].map((p) => p.close()));
    for (const p of this.panes.values()) p.dispose();
    this.panes.clear();
    for (const f of [this.socketPath, this.pidPath]) fs.rmSync(f, { force: true });
  }

  // ---- 接続 / ディスパッチ ----

  private accept(socket: net.Socket): void {
    const conn = new Connection(
      socket,
      (c, msg, err) => void this.onMessage(c, msg, err),
      (c) => this.onConnClose(c),
    );
    this.conns.add(conn);
  }

  private onConnClose(conn: Connection): void {
    this.conns.delete(conn);
    for (const paneId of [...conn.attachments.keys()]) this.detach(conn, paneId);
  }

  private async onMessage(conn: Connection, msg: RpcMessage | undefined, parseError?: string): Promise<void> {
    if (!msg) {
      conn.send({ jsonrpc: '2.0', id: null, error: { code: ErrorCode.Parse, message: parseError ?? 'parse error' } });
      return;
    }
    if (!isRequest(msg)) return; // クライアントからの通知・応答は無視
    const after: Array<() => void> = [];
    try {
      const result = await this.dispatch(msg, { conn, afterReply: (fn) => after.push(fn) });
      conn.send({ jsonrpc: '2.0', id: msg.id, result: result ?? null });
    } catch (e) {
      const code = e instanceof RpcFailure ? e.code : ErrorCode.Internal;
      const message = (e as Error).message;
      if (!(e instanceof RpcFailure)) this.log(`internal error in ${msg.method}: ${(e as Error).stack}`);
      conn.send({ jsonrpc: '2.0', id: msg.id, error: { code, message } });
      return;
    }
    for (const fn of after) fn();
  }

  private dispatch(req: RpcRequest, ctx: Ctx): Promise<unknown> | unknown {
    const handler = this.handlers[req.method];
    if (!handler) throw new RpcFailure(ErrorCode.MethodNotFound, `unknown method: ${req.method}`);
    return handler(req.params ?? {}, ctx);
  }

  // ---- events ----

  private emitEvent(type: string, data: Record<string, unknown>): void {
    const ts = nowIso();
    const seq = this.eventRing.push({ type, data }, 1, ts);
    this.bus.emit('event', seq, { type, data }, ts);
  }

  // ---- RPC 実装 ----

  private serverInfo(): unknown {
    return {
      version: PROTOCOL_VERSION,
      pid: process.pid,
      epoch: this.epoch,
      uptimeSec: Math.floor((Date.now() - this.startedAt) / 1000),
      paneCount: this.panes.size,
      eventHead: this.eventRing.head,
    };
  }

  private pane(p: Params): Pane {
    const id = str(p, 'paneId');
    const pane = this.panes.get(id);
    if (!pane) throw new RpcFailure(ErrorCode.PaneNotFound, `pane not found: ${id}`);
    return pane;
  }

  private paneOpen(p: Params): unknown {
    const cmd = p.cmd;
    if (!Array.isArray(cmd) || cmd.length === 0 || !cmd.every((c) => typeof c === 'string')) {
      throw new RpcFailure(ErrorCode.InvalidParams, 'cmd must be a non-empty string[]');
    }
    let pane: Pane;
    try {
      pane = new Pane({
        cmd: cmd as string[],
        cwd: optStr(p, 'cwd'),
        env: p.env as Record<string, string> | undefined,
        cols: optInt(p, 'cols'),
        rows: optInt(p, 'rows'),
        labels: p.labels as Record<string, string> | undefined,
        socketPath: this.socketPath,
      });
    } catch (e) {
      throw new RpcFailure(ErrorCode.InvalidParams, `spawn failed: ${(e as Error).message}`);
    }
    this.panes.set(pane.id, pane);
    pane.on('title', (title: string) => this.emitEvent('pane.title', { paneId: pane.id, title }));
    pane.on('exit', (r: { exitCode: number | null; signal: number | null }) =>
      this.emitEvent('pane.exited', { paneId: pane.id, ...r }),
    );
    this.emitEvent('pane.opened', { paneId: pane.id, pid: pane.pid, cmd: pane.cmd, labels: pane.labels });
    return { paneId: pane.id };
  }

  private paneWrite(p: Params): unknown {
    const pane = this.pane(p);
    if (pane.state === 'exited') throw new RpcFailure(ErrorCode.PaneExited, 'pane has exited');
    const data =
      typeof p.dataB64 === 'string'
        ? Buffer.from(p.dataB64, 'base64')
        : typeof p.data === 'string'
          ? Buffer.from(p.data, 'utf8')
          : undefined;
    if (!data) throw new RpcFailure(ErrorCode.InvalidParams, 'data or dataB64 required');
    const source = p.source === 'terminal' ? 'terminal' : 'hub';
    pane.write(data);
    this.emitEvent('input', { paneId: pane.id, source, bytes: data.length }); // 内容は記録しない
    return { ok: true };
  }

  private paneResize(p: Params): unknown {
    const pane = this.pane(p);
    const cols = reqInt(p, 'cols');
    const rows = reqInt(p, 'rows');
    const clientId = optStr(p, 'clientId') ?? null;
    pane.resize(cols, rows);
    this.emitEvent('pane.resized', { paneId: pane.id, cols, rows, clientId });
    return { ok: true };
  }

  private async paneAttach(p: Params, ctx: Ctx): Promise<unknown> {
    const pane = this.pane(p);
    const clientId = str(p, 'clientId');
    const replay = optStr(p, 'replay') ?? 'none';
    if (!['raw', 'snapshot', 'none'].includes(replay)) {
      throw new RpcFailure(ErrorCode.InvalidParams, 'replay must be raw|snapshot|none');
    }
    const conn = ctx.conn;
    this.detach(conn, pane.id);

    const queue: Array<[number, Buffer, string]> = [];
    let live = false;
    const send = (seq: number, data: Buffer, ts: string) =>
      conn.notify('pane.output', seq, ts, { paneId: pane.id, dataB64: data.toString('base64') });
    const listener = (seq: number, data: Buffer, ts: string) => (live ? send(seq, data, ts) : queue.push([seq, data, ts]));
    pane.on('output', listener);
    const off = () => pane.off('output', listener);
    conn.attachments.set(pane.id, { clientId, off });
    pane.clients.add(clientId);
    this.emitEvent('client.attached', { paneId: pane.id, clientId });

    let headSeq = pane.rawRing.head;
    let snapshot: string | undefined;
    if (replay === 'snapshot') {
      const snap = await pane.snapshot();
      headSeq = snap.headSeq;
      snapshot = snap.data;
    }
    ctx.afterReply(() => {
      if (replay === 'raw') {
        for (const e of pane.rawRing.since(0)) if (e.seq <= headSeq) send(e.seq, e.item, e.ts);
      } else if (snapshot !== undefined) {
        conn.notify('pane.output', headSeq, nowIso(), {
          paneId: pane.id,
          dataB64: Buffer.from(snapshot, 'utf8').toString('base64'),
          replay: 'snapshot',
        });
      }
      for (const [seq, data, ts] of queue) if (seq > headSeq) send(seq, data, ts);
      queue.length = 0;
      live = true;
    });
    return { headSeq };
  }

  private paneDetach(p: Params, ctx: Ctx): unknown {
    this.detach(ctx.conn, str(p, 'paneId'));
    return { ok: true };
  }

  private detach(conn: Connection, paneId: string): void {
    const att = conn.attachments.get(paneId);
    if (!att) return;
    att.off();
    conn.attachments.delete(paneId);
    const pane = this.panes.get(paneId);
    // 同じ clientId が別接続にも残っていれば clients から外さない
    const stillAttached = [...this.conns].some((c) => c !== conn && c.attachments.get(paneId)?.clientId === att.clientId);
    if (pane && !stillAttached) pane.clients.delete(att.clientId);
    this.emitEvent('client.detached', { paneId, clientId: att.clientId });
  }

  private subscribeLines(p: Params, ctx: Ctx): unknown {
    const pane = this.pane(p);
    return this.subscribe(ctx, pane.linesRing, optSince(p), (cb) => {
      pane.on('line', cb);
      return () => pane.off('line', cb);
    }, (seq, text, ts) => ctx.conn.notify('pane.line', seq, ts, { paneId: pane.id, text }));
  }

  private subscribeEvents(p: Params, ctx: Ctx): unknown {
    return this.subscribe(ctx, this.eventRing, optSince(p), (cb) => {
      this.bus.on('event', cb);
      return () => this.bus.off('event', cb);
    }, (seq, ev, ts) => ctx.conn.notify('event', seq, ts, { type: ev.type, data: ev.data }));
  }

  /** ring の since 以降を再生してからライブに切り替える共通処理。since 省略時はライブのみ。 */
  private subscribe<T>(
    ctx: Ctx,
    ring: SeqRing<T>,
    since: number | undefined,
    on: (cb: (seq: number, item: T, ts: string) => void) => () => void,
    notify: (seq: number, item: T, ts: string) => void,
  ): unknown {
    const head = ring.head;
    const gap = since !== undefined && ring.hasGap(since);
    const queue: Array<[number, T, string]> = [];
    let live = false;
    const off = on((seq, item, ts) => (live ? notify(seq, item, ts) : queue.push([seq, item, ts])));
    ctx.conn.onClose(off);
    ctx.afterReply(() => {
      if (since !== undefined) for (const e of ring.since(since)) if (e.seq <= head) notify(e.seq, e.item, e.ts);
      for (const [seq, item, ts] of queue) if (seq > head) notify(seq, item, ts);
      queue.length = 0;
      live = true;
    });
    return { gap, head, epoch: this.epoch };
  }

  private async paneClose(p: Params): Promise<unknown> {
    const pane = this.pane(p);
    await pane.close();
    this.panes.delete(pane.id);
    this.emitEvent('pane.closed', { paneId: pane.id });
    pane.dispose();
    return { ok: true };
  }
}

// ---- params helpers ----

function str(p: Params, key: string): string {
  const v = p[key];
  if (typeof v !== 'string' || v === '') throw new RpcFailure(ErrorCode.InvalidParams, `${key} (string) required`);
  return v;
}
function optStr(p: Params, key: string): string | undefined {
  const v = p[key];
  if (v === undefined || v === null) return undefined;
  if (typeof v !== 'string') throw new RpcFailure(ErrorCode.InvalidParams, `${key} must be a string`);
  return v;
}
function optInt(p: Params, key: string): number | undefined {
  const v = p[key];
  if (v === undefined || v === null) return undefined;
  if (!Number.isInteger(v) || (v as number) <= 0) throw new RpcFailure(ErrorCode.InvalidParams, `${key} must be a positive integer`);
  return v as number;
}
function reqInt(p: Params, key: string): number {
  const v = optInt(p, key);
  if (v === undefined) throw new RpcFailure(ErrorCode.InvalidParams, `${key} (integer) required`);
  return v;
}
function optSince(p: Params): number | undefined {
  const v = p.since;
  if (v === undefined || v === null) return undefined;
  if (!Number.isInteger(v) || (v as number) < 0) throw new RpcFailure(ErrorCode.InvalidParams, 'since must be a non-negative integer');
  return v as number;
}
