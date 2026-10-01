import * as fs from 'node:fs';
import * as net from 'node:net';
import {
  ErrorCode,
  PROTOCOL_VERSION,
  RpcRequestSchema,
  buildProtocolJsonSchema,
  methods,
} from '@misao/protocol';
import type { PaneInfo, RpcRequest } from '@misao/protocol';
import { nowIso } from './clock.js';
import { attachPane } from './attach.js';
import { Connection } from './connection.js';
import { EventLog } from './event-log.js';
import { parseParams } from './params.js';
import type { ParsedParams } from './params.js';
import { Pane } from './pane.js';
import { RpcFailure } from './rpc-error.js';
import { ensureSocketDir, listenUnixSocket } from './socket.js';
import { subscribeStream } from './stream.js';
import type { RequestContext } from './stream.js';
import { newWindowId, ulid } from './ulid.js';

/** 仮のモデル。workspace / window の実体は #5 で置き換える。 */
const DEFAULT_WORKSPACE = 'default';
const DEFAULT_WINDOW_NAME = 'default';

type ImplementedMethod =
  | 'server.info'
  | 'server.schema'
  | 'pane.open'
  | 'pane.info'
  | 'pane.list'
  | 'pane.write'
  | 'pane.resize'
  | 'pane.screen'
  | 'pane.attach'
  | 'pane.detach'
  | 'pane.subscribe_lines'
  | 'pane.close'
  | 'events.subscribe';

type Handlers = {
  [M in ImplementedMethod]: (params: ParsedParams<M>, ctx: RequestContext) => unknown;
};

export interface DaemonOptions {
  socketPath: string;
  pidPath: string;
  log?: (msg: string) => void;
}

export class Daemon {
  readonly socketPath: string;
  private readonly pidPath: string;
  private readonly log: (msg: string) => void;
  private readonly panes = new Map<string, Pane>();
  private readonly events = new EventLog();
  private readonly conns = new Set<Connection>();
  private server: net.Server | undefined;
  private readonly startedAt = Date.now();
  /** 起動ごとに変わる ID。seq の巻き戻り (デーモン再起動) をクライアントが検知するためのもの。 */
  private readonly epoch = ulid();
  private readonly window = { id: newWindowId(), name: DEFAULT_WINDOW_NAME };
  private readonly handlers: Handlers;

  constructor(opts: DaemonOptions) {
    this.socketPath = opts.socketPath;
    this.pidPath = opts.pidPath;
    this.log = opts.log ?? ((m) => process.stderr.write(`[misao ${nowIso()}] ${m}\n`));
    this.handlers = {
      'server.info': () => this.serverInfo(),
      'server.schema': () => buildProtocolJsonSchema(),
      'pane.open': (p) => this.paneOpen(p),
      'pane.info': (p) => this.pane(p.paneId).info(DEFAULT_WORKSPACE, this.window),
      'pane.list': (p) => this.paneList(p),
      'pane.write': (p) => this.paneWrite(p),
      'pane.resize': (p) => this.paneResize(p),
      'pane.screen': (p) => this.pane(p.paneId).screen(),
      'pane.attach': (p, ctx) => this.paneAttach(p, ctx),
      'pane.detach': (p, ctx) => this.paneDetach(p, ctx),
      'pane.subscribe_lines': (p, ctx) => this.subscribeLines(p, ctx),
      'events.subscribe': (p, ctx) => this.subscribeEvents(p, ctx),
      'pane.close': (p) => this.paneClose(p),
    };
  }

  async start(): Promise<void> {
    await ensureSocketDir(this.socketPath);
    await this.removeStaleSocket();
    this.server = net.createServer((socket) => this.accept(socket));
    await listenUnixSocket(this.server, this.socketPath);
    // listen 後の accept 失敗 (EMFILE など) でデーモンごと落ちないよう、その接続だけを諦める。
    this.server.on('error', (e) => this.log(`server error: ${e.message}`));
    fs.writeFileSync(this.pidPath, String(process.pid));
    this.events.emit('daemon.started', { pid: process.pid, protocolVersion: PROTOCOL_VERSION });
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
    const server = this.server;
    const closed = new Promise<void>((resolve) => (server ? server.close(() => resolve()) : resolve()));
    for (const conn of this.conns) conn.socket.destroy();
    await closed;
    await Promise.all([...this.panes.values()].map((p) => p.close()));
    for (const p of this.panes.values()) p.dispose();
    this.panes.clear();
    for (const f of [this.socketPath, this.pidPath]) fs.rmSync(f, { force: true });
  }

  // ---- 接続 / ディスパッチ ----

  private accept(socket: net.Socket): void {
    const conn = new Connection(socket, {
      onMessage: (c, value) => void this.onMessage(c, value),
      onClose: (c) => this.onConnClose(c),
    });
    this.conns.add(conn);
  }

  private onConnClose(conn: Connection): void {
    this.conns.delete(conn);
    for (const paneId of [...conn.attachments.keys()]) this.detach(conn, paneId);
  }

  private async onMessage(conn: Connection, value: unknown): Promise<void> {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) {
      this.sendError(conn, null, ErrorCode.InvalidRequest, 'invalid request');
      return;
    }
    if (!('method' in value && 'id' in value)) return; // クライアントからの通知・応答は無視
    const parsed = RpcRequestSchema.safeParse(value);
    if (!parsed.success) {
      const id = typeof value.id === 'string' || typeof value.id === 'number' ? value.id : null;
      this.sendError(conn, id, ErrorCode.InvalidRequest, 'invalid request');
      return;
    }
    const req = parsed.data;
    const after: Array<() => void> = [];
    try {
      const result = await this.dispatch(req, { conn, afterReply: (fn) => after.push(fn) });
      conn.send({ jsonrpc: '2.0', id: req.id, result: result ?? null });
    } catch (e) {
      if (e instanceof RpcFailure) {
        this.sendError(conn, req.id, e.code, e.message);
      } else {
        this.log(`internal error in ${req.method}: ${(e as Error).stack}`);
        this.sendError(conn, req.id, ErrorCode.Internal, (e as Error).message);
      }
      return;
    }
    for (const fn of after) fn();
  }

  private sendError(conn: Connection, id: RpcRequest['id'] | null, code: number, message: string): void {
    conn.send({ jsonrpc: '2.0', id, error: { code, message } });
  }

  private dispatch(req: RpcRequest, ctx: RequestContext): unknown {
    const method = req.method;
    if (!Object.hasOwn(methods, method)) {
      throw new RpcFailure(ErrorCode.MethodNotFound, `unknown method: ${method}`);
    }
    if (!Object.hasOwn(this.handlers, method)) {
      throw new RpcFailure(ErrorCode.NotImplemented, `not implemented: ${method}`);
    }
    const name = method as ImplementedMethod;
    const handler = this.handlers[name] as (params: unknown, ctx: RequestContext) => unknown;
    return handler(parseParams(name, req.params), ctx);
  }

  // ---- RPC 実装 ----

  private serverInfo(): unknown {
    return {
      protocolVersion: PROTOCOL_VERSION,
      pid: process.pid,
      epoch: this.epoch,
      uptimeSec: Math.floor((Date.now() - this.startedAt) / 1000),
      paneCount: this.panes.size,
      eventHead: this.events.head,
    };
  }

  private pane(id: string): Pane {
    const pane = this.panes.get(id);
    if (!pane) throw new RpcFailure(ErrorCode.PaneNotFound, `pane not found: ${id}`);
    return pane;
  }

  private paneOpen(p: ParsedParams<'pane.open'>): unknown {
    if (p.preplace !== undefined) throw new RpcFailure(ErrorCode.NotImplemented, 'preplace is not implemented');
    if (p.windowId !== undefined && p.windowId !== this.window.id) {
      throw new RpcFailure(ErrorCode.WindowNotFound, `window not found: ${p.windowId}`);
    }
    let pane: Pane;
    try {
      pane = new Pane({
        cmd: p.cmd,
        cwd: p.cwd,
        env: p.env,
        cols: p.cols,
        rows: p.rows,
        labels: p.labels,
        socketPath: this.socketPath,
      });
    } catch (e) {
      throw new RpcFailure(ErrorCode.InvalidParams, `spawn failed: ${(e as Error).message}`);
    }
    this.panes.set(pane.id, pane);
    pane.on('title', (title: string) => this.events.emit('pane.title', { title }, pane.id));
    pane.on('exit', (r: { exitCode: number | null; signal: number | null }) =>
      this.events.emit('pane.exited', { ...r }, pane.id),
    );
    this.events.emit(
      'pane.opened',
      { pid: pane.pid, cmd: pane.cmd, labels: pane.labels, workspace: DEFAULT_WORKSPACE, windowId: this.window.id },
      pane.id,
    );
    return { paneId: pane.id };
  }

  private paneList(p: ParsedParams<'pane.list'>): PaneInfo[] {
    const { state, labels, workspace } = p.filter ?? {};
    if (workspace !== undefined && workspace !== DEFAULT_WORKSPACE) return [];
    return [...this.panes.values()]
      .filter((pane) => state === undefined || pane.state === state)
      .filter((pane) => labels === undefined || Object.entries(labels).every(([k, v]) => pane.labels[k] === v))
      .map((pane) => pane.info(DEFAULT_WORKSPACE, this.window));
  }

  private paneWrite(p: ParsedParams<'pane.write'>): unknown {
    const pane = this.pane(p.paneId);
    if (pane.state === 'exited') throw new RpcFailure(ErrorCode.PaneExited, 'pane has exited');
    const data = p.dataB64 !== undefined ? Buffer.from(p.dataB64, 'base64') : Buffer.from(p.data, 'utf8');
    // 最後に操作したクライアントのサイズを優先する
    if (p.clientId && pane.claimSize(p.clientId)) {
      this.events.emit('pane.resized', { cols: pane.cols, rows: pane.rows, clientId: p.clientId }, pane.id);
    }
    pane.write(data);
    this.events.emit('input', { source: p.source ?? 'hub', bytes: data.length }, pane.id); // 内容は記録しない
    return { ok: true };
  }

  private paneResize(p: ParsedParams<'pane.resize'>): unknown {
    const pane = this.pane(p.paneId);
    const clientId = p.clientId ?? null;
    pane.resize(p.cols, p.rows, clientId);
    this.events.emit('pane.resized', { cols: p.cols, rows: p.rows, clientId }, pane.id);
    return { ok: true };
  }

  private async paneAttach(p: ParsedParams<'pane.attach'>, ctx: RequestContext): Promise<unknown> {
    if (p.mode === 'cells') throw new RpcFailure(ErrorCode.Unsupported, 'mode "cells" is not supported');
    const pane = this.pane(p.paneId);
    this.detach(ctx.conn, pane.id);
    if (p.cols !== undefined && p.rows !== undefined) {
      pane.resize(p.cols, p.rows, p.clientId);
      this.events.emit('pane.resized', { cols: p.cols, rows: p.rows, clientId: p.clientId }, pane.id);
    }
    // attachPane は最初の await までに購読と clients への登録を同期的に済ませる
    const attached = attachPane(ctx, pane, p.clientId, p.replay);
    this.events.emit('client.attached', { clientId: p.clientId }, pane.id);
    return attached;
  }

  private paneDetach(p: ParsedParams<'pane.detach'>, ctx: RequestContext): unknown {
    this.detach(ctx.conn, p.paneId);
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
    if (pane && !stillAttached) {
      pane.clients.delete(att.clientId);
      const inherited = pane.forgetClient(att.clientId);
      if (inherited) {
        this.events.emit('pane.resized', { cols: pane.cols, rows: pane.rows, clientId: inherited }, paneId);
      }
    }
    this.events.emit('client.detached', { clientId: att.clientId }, paneId);
  }

  private subscribeLines(p: ParsedParams<'pane.subscribe_lines'>, ctx: RequestContext): unknown {
    const pane = this.pane(p.paneId);
    return subscribeStream({
      ctx,
      key: `lines:${pane.id}`,
      ring: pane.linesRing,
      on: (cb) => {
        pane.on('line', cb);
        return () => pane.off('line', cb);
      },
      notify: (seq, text, ts) => ctx.conn.notify('pane.line', seq, ts, { paneId: pane.id, text }),
      since: p.since,
      epoch: p.epoch,
      currentEpoch: this.epoch,
    });
  }

  private subscribeEvents(p: ParsedParams<'events.subscribe'>, ctx: RequestContext): unknown {
    return subscribeStream({
      ctx,
      key: 'events',
      ring: this.events.ring,
      on: (cb) => this.events.on(cb),
      notify: (seq, ev, ts) =>
        ctx.conn.notify('event', seq, ts, {
          type: ev.type,
          data: ev.data,
          ...(ev.paneId === undefined ? {} : { paneId: ev.paneId }),
        }),
      since: p.since,
      epoch: p.epoch,
      currentEpoch: this.epoch,
    });
  }

  private async paneClose(p: ParsedParams<'pane.close'>): Promise<unknown> {
    const pane = this.pane(p.paneId);
    await pane.close();
    if (this.panes.get(pane.id) !== pane) return { ok: true }; // 並行した close が後始末済み
    this.panes.delete(pane.id);
    this.events.emit('pane.closed', {}, pane.id);
    pane.dispose();
    return { ok: true };
  }
}
