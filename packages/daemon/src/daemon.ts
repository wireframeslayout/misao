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
import { acquirePidFile } from './pid-file.js';
import { Layout } from './layout.js';
import type { ResolvedWindow } from './layout.js';
import { createLayoutHandlers } from './layout-handlers.js';
import type { LayoutMethod } from './layout-handlers.js';
import { PaneRegistry } from './pane-registry.js';
import { loadPersistedState, savePersistedState } from './persistence.js';
import { ensureSocketDir, listenUnixSocket, removeStaleSocket } from './socket.js';
import { subscribeStream } from './stream.js';
import type { RequestContext } from './stream.js';
import { ulid } from './ulid.js';

/** 接続ごとの購読キー。同じキーの再購読は既存を置き換える。 */
const linesKey = (paneId: string): string => `lines:${paneId}`;

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
  | 'pane.set_label'
  | 'pane.close'
  | 'events.subscribe'
  | LayoutMethod;

type Handlers = {
  [M in ImplementedMethod]: (params: ParsedParams<M>, ctx: RequestContext) => unknown;
};

export interface DaemonOptions {
  socketPath: string;
  pidPath: string;
  /** persistence.json の場所。 */
  statePath: string;
  log?: (msg: string) => void;
}

export class Daemon {
  readonly socketPath: string;
  private readonly pidPath: string;
  private readonly statePath: string;
  private readonly log: (msg: string) => void;
  private readonly registry = new PaneRegistry();
  private readonly layout = new Layout();
  private readonly events = new EventLog();
  private readonly conns = new Set<Connection>();
  private server: net.Server | undefined;
  private readonly startedAt = Date.now();
  /** 起動ごとに変わる ID。seq の巻き戻り (デーモン再起動) をクライアントが検知するためのもの。 */
  private readonly epoch = ulid();
  private readonly handlers: Handlers;

  constructor(opts: DaemonOptions) {
    this.socketPath = opts.socketPath;
    this.pidPath = opts.pidPath;
    this.statePath = opts.statePath;
    this.log = opts.log ?? ((m) => process.stderr.write(`[misao ${nowIso()}] ${m}\n`));
    this.handlers = {
      'server.info': () => this.serverInfo(),
      'server.schema': () => buildProtocolJsonSchema(),
      'pane.open': (p) => this.paneOpen(p),
      'pane.info': (p) => this.info(p.paneId),
      'pane.list': (p) => this.paneList(p),
      'pane.write': (p) => this.paneWrite(p),
      'pane.resize': (p) => this.paneResize(p),
      'pane.screen': (p) => this.livePane(p.paneId).screen(),
      'pane.attach': (p, ctx) => this.paneAttach(p, ctx),
      'pane.detach': (p, ctx) => this.paneDetach(p, ctx),
      'pane.subscribe_lines': (p, ctx) => this.subscribeLines(p, ctx),
      'events.subscribe': (p, ctx) => this.subscribeEvents(p, ctx),
      'pane.set_label': (p) => this.paneSetLabel(p),
      'pane.close': async (p) => {
        await this.closePane(p.paneId);
        return { ok: true };
      },
      ...createLayoutHandlers({
        layout: this.layout,
        events: this.events,
        closePane: (id) => this.closePane(id),
        paneIdsIn: (windowIds) => this.registry.filter(undefined, undefined, windowIds).map((e) => e.record.paneId),
        persist: () => this.persist(),
        commit: (mutate) => this.commitLayout(mutate),
      }),
    };
  }

  async start(): Promise<void> {
    await ensureSocketDir(this.socketPath);
    acquirePidFile(this.pidPath, process.pid);
    // pid ファイルで排他を取った後は、socketPath にあるファイルを自分のものとして扱える。
    try {
      this.loadState();
      if (await removeStaleSocket(this.socketPath)) this.log('removed stale socket');
    } catch (e) {
      fs.rmSync(this.pidPath, { force: true });
      throw e;
    }
    try {
      this.server = net.createServer((socket) => this.accept(socket));
      await listenUnixSocket(this.server, this.socketPath);
      // listen 後の accept 失敗 (EMFILE など) でデーモンごと落ちないよう、その接続だけを諦める。
      this.server.on('error', (e) => this.log(`server error: ${e.message}`));
    } catch (e) {
      this.server?.close();
      for (const f of [this.socketPath, this.pidPath]) fs.rmSync(f, { force: true });
      throw e;
    }
    this.events.emit('daemon.started', { pid: process.pid, protocolVersion: PROTOCOL_VERSION });
    this.log(`listening on ${this.socketPath} (pid ${process.pid})`);
  }

  async shutdown(): Promise<void> {
    this.log('shutting down');
    // close() は接続が全部閉じるまで完了しないので、先に接続を切ってから待つ。
    const server = this.server;
    const closed = new Promise<void>((resolve) => (server ? server.close(() => resolve()) : resolve()));
    for (const conn of this.conns) conn.socket.destroy();
    await closed;
    // record は消さず保存もしない: 再起動後に stopped として復元される。
    const live = this.registry.livePanes();
    await Promise.all(live.map((p) => p.close()));
    for (const p of live) {
      this.releasePane(p.id);
      p.dispose();
    }
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
      paneCount: this.registry.size,
      eventHead: this.events.head,
    };
  }

  private loadState(): void {
    const state = loadPersistedState(this.statePath);
    this.layout.restore(state.workspaces);
    this.registry.restoreStopped(state.panes);
  }

  /** layout を変更して保存する。保存に失敗したら layout を戻して例外を伝播させる。 */
  private commitLayout<T>(mutate: () => T): T {
    const before = this.layout.toPersisted();
    try {
      const result = mutate();
      this.persist();
      return result;
    } catch (e) {
      this.layout.restore(before);
      throw e;
    }
  }

  /** 状態を変えたら、イベントを出す前に呼ぶ。失敗は握りつぶさず RPC エラーにする。 */
  private persist(): void {
    savePersistedState(this.statePath, {
      version: 1,
      workspaces: this.layout.toPersisted(),
      panes: this.registry.toPersisted(),
    });
  }

  /** 実行体のある pane。stopped (再起動後の復元分) は PaneExited。 */
  private livePane(id: string): Pane {
    const { live } = this.registry.getOrThrow(id);
    if (!live) throw new RpcFailure(ErrorCode.PaneExited, 'pane is stopped');
    return live;
  }

  private info(paneId: string): PaneInfo {
    const { workspace, window } = this.layout.windowRef(this.registry.getOrThrow(paneId).record.windowId);
    return this.registry.info(paneId, workspace, window);
  }

  private spawnPane(p: ParsedParams<'pane.open'>): Pane {
    try {
      return new Pane({
        cmd: p.cmd,
        cwd: p.cwd,
        env: p.env,
        ephemeralEnv: p.ephemeralEnv,
        cols: p.cols,
        rows: p.rows,
        socketPath: this.socketPath,
      });
    } catch (e) {
      throw new RpcFailure(ErrorCode.InvalidParams, `spawn failed: ${(e as Error).message}`);
    }
  }

  /** 既定の workspace / window を新しく作っていたら、保存した後にそのイベントを出す。 */
  private emitCreatedDefaults({ workspace, window, createdWorkspace, createdWindow }: ResolvedWindow): void {
    if (createdWorkspace) this.events.emit('workspace.created', { name: workspace });
    if (createdWindow) this.events.emit('window.created', { windowId: window.id, workspace, name: window.name });
  }

  /**
   * layout (既定の遅延作成を含む) → spawn → record 登録 → 保存、を 1 つの単位として扱う。
   * 保存まで失敗したら全て戻し、起動した子プロセスも止め、イベントは一切出さない。
   */
  private async paneOpen(p: ParsedParams<'pane.open'>): Promise<unknown> {
    if (p.preplace !== undefined) throw new RpcFailure(ErrorCode.NotImplemented, 'preplace is not implemented');
    const before = this.layout.toPersisted();
    const target = this.layout.resolveWindow(p.windowId);
    const labels = p.labels ?? {};
    let pane: Pane | undefined;
    try {
      pane = this.spawnPane(p);
      // 保存するのは env のみ。ephemeralEnv は Pane の spawn で使い切り、ここには渡らない。
      const record = { paneId: pane.id, windowId: target.window.id, cmd: p.cmd, cwd: pane.cwd, env: p.env ?? {}, labels, cols: pane.cols, rows: pane.rows };
      this.registry.add(record, pane);
      this.persist();
    } catch (e) {
      this.layout.restore(before);
      if (pane) {
        this.registry.remove(pane.id);
        await pane.close();
        pane.dispose();
      }
      throw e;
    }
    const live = pane;
    live.on('title', (title: string) => this.events.emit('pane.title', { title }, live.id));
    live.on('exit', (r: { exitCode: number | null; signal: number | null }) =>
      this.events.emit('pane.exited', { ...r }, live.id),
    );
    this.emitCreatedDefaults(target);
    this.events.emit(
      'pane.opened',
      { pid: live.pid, cmd: live.cmd, labels, workspace: target.workspace, windowId: target.window.id },
      live.id,
    );
    return { paneId: live.id };
  }

  private paneList(p: ParsedParams<'pane.list'>): PaneInfo[] {
    const { state, labels, workspace } = p.filter ?? {};
    const windowIds = workspace === undefined ? undefined : new Set(this.layout.windowIds(workspace));
    return this.registry.filter(state, labels, windowIds).map((e) => this.info(e.record.paneId));
  }

  private paneSetLabel(p: ParsedParams<'pane.set_label'>): unknown {
    const before = this.registry.getOrThrow(p.paneId).record;
    const labels = this.registry.setLabels(p.paneId, p.set, p.unset);
    try {
      this.persist();
    } catch (e) {
      this.registry.replaceRecord(before);
      throw e;
    }
    this.events.emit('pane.label', { set: p.set ?? {}, unset: p.unset ?? [] }, p.paneId);
    return { labels };
  }

  private paneWrite(p: ParsedParams<'pane.write'>): unknown {
    const pane = this.livePane(p.paneId);
    if (pane.state === 'exited') throw new RpcFailure(ErrorCode.PaneExited, 'pane has exited');
    const data = p.dataB64 !== undefined ? Buffer.from(p.dataB64, 'base64') : Buffer.from(p.data, 'utf8');
    // 最後に操作したクライアントのサイズを優先する
    if (p.clientId && pane.claimSize(p.clientId)) {
      this.persist();
      this.events.emit('pane.resized', { cols: pane.cols, rows: pane.rows, clientId: p.clientId }, pane.id);
    }
    pane.write(data);
    this.events.emit('input', { source: p.source ?? 'hub', bytes: data.length }, pane.id); // 内容は記録しない
    return { ok: true };
  }

  private paneResize(p: ParsedParams<'pane.resize'>): unknown {
    const pane = this.livePane(p.paneId);
    const clientId = p.clientId ?? null;
    pane.resize(p.cols, p.rows, clientId);
    this.persist();
    this.events.emit('pane.resized', { cols: p.cols, rows: p.rows, clientId }, pane.id);
    return { ok: true };
  }

  private async paneAttach(p: ParsedParams<'pane.attach'>, ctx: RequestContext): Promise<unknown> {
    if (p.mode === 'cells') throw new RpcFailure(ErrorCode.Unsupported, 'mode "cells" is not supported');
    const pane = this.livePane(p.paneId);
    const replacing = ctx.conn.attachments.get(pane.id)?.clientId === p.clientId;
    if (replacing) this.replaceAttachment(ctx.conn, pane.id);
    else this.detach(ctx.conn, pane.id);
    if (p.cols !== undefined && p.rows !== undefined) {
      pane.resize(p.cols, p.rows, p.clientId);
      this.persist();
      this.events.emit('pane.resized', { cols: p.cols, rows: p.rows, clientId: p.clientId }, pane.id);
    }
    // attachPane は最初の await までに購読と clients への登録を同期的に済ませる
    const attached = attachPane(ctx, pane, p.clientId, p.replay);
    if (!replacing) this.events.emit('client.attached', { clientId: p.clientId }, pane.id);
    return attached;
  }

  /**
   * 同じ接続・同じ clientId の再 attach 用。出力の購読だけを外し、クライアントとしては残す
   * (clients・サイズの記録と所有権は保ち、client.detached も出さない)。
   */
  private replaceAttachment(conn: Connection, paneId: string): void {
    conn.attachments.get(paneId)?.off();
    conn.attachments.delete(paneId);
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
    const pane = this.registry.get(paneId)?.live;
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
    const pane = this.livePane(p.paneId);
    return subscribeStream({
      ctx,
      key: linesKey(pane.id),
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

  /**
   * live は SIGHUP → 終了待ち、stopped は record 削除のみ。どちらも保存してから pane.closed を出す。
   * live は保存に失敗してもプロセスを戻せないので、メモリは実態 (閉じ済み) に合わせたまま例外を伝播させる。
   * stopped はプロセスに触れていないので、保存に失敗したら record を戻す。
   */
  private async closePane(paneId: string): Promise<void> {
    const { record, live } = this.registry.getOrThrow(paneId);
    if (live) {
      await live.close();
      if (this.registry.get(paneId)?.live !== live) return; // 並行した close が後始末済み
      this.releasePane(paneId); // client.detached は pane.closed より前に出す
    }
    this.registry.remove(paneId);
    live?.dispose();
    try {
      this.persist();
    } catch (e) {
      if (!live) this.registry.restoreStopped([record]);
      throw e;
    }
    this.events.emit('pane.closed', {}, paneId);
  }

  /** 全接続から、この pane の attachment と行購読を外す (閉じた Pane への参照を残さない)。 */
  private releasePane(paneId: string): void {
    const key = linesKey(paneId);
    for (const conn of this.conns) {
      this.detach(conn, paneId);
      conn.subscriptions.get(key)?.();
      conn.subscriptions.delete(key);
    }
  }
}
