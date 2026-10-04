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
import { Connection } from './connection.js';
import { EventLog } from './event-log.js';
import { resolveDaemonLimits } from './limits.js';
import type { DaemonLimitOptions } from './limits.js';
import { DEFAULT_LOG_LEVEL, createLogger } from './log.js';
import type { LogLevel, Logger } from './log.js';
import { parseParams } from './params.js';
import type { ParsedParams } from './params.js';
import { RpcFailure } from './rpc-error.js';
import { acquirePidFile } from './pid-file.js';
import { Layout } from './layout.js';
import { createLayoutHandlers } from './layout-handlers.js';
import type { LayoutMethod } from './layout-handlers.js';
import { PaneRegistry } from './pane-registry.js';
import { PaneIo } from './pane-io.js';
import { PaneLifecycle } from './pane-lifecycle.js';
import type { AgentProfile } from './profile.js';
import { loadPersistedState, savePersistedState } from './persistence.js';
import type { PersistedState } from './persistence.js';
import { StatePersister } from './state-persister.js';
import { ensureSocketDir, listenUnixSocket, removeStaleSocket } from './socket.js';
import { subscribeStream } from './stream.js';
import type { RequestContext } from './stream.js';
import { ulid } from './ulid.js';

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

export interface DaemonOptions extends DaemonLimitOptions {
  /** デーモンのビルド版 (リリースの version)。server.info の version で報告する。 */
  version: string;
  socketPath: string;
  pidPath: string;
  /** persistence.json の場所。 */
  statePath: string;
  /** 保存の実体。省略時は savePersistedState (ファイルへ原子的に書く)。 */
  saveState?: (path: string, state: PersistedState) => void;
  /** ログの出力先。省略時は stderr。閾値は logLevel で決める。 */
  log?: (msg: string) => void;
  /** ログの閾値 (error < warn < info < debug)。省略時は info。 */
  logLevel?: LogLevel;
  /** 稼働判定のプロファイル (Claude / Codex など)。省略時は汎用の判定だけ。 */
  profiles?: readonly AgentProfile[];
}

export class Daemon {
  private readonly version: string;
  readonly socketPath: string;
  private readonly pidPath: string;
  private readonly statePath: string;
  private readonly log: Logger;
  private readonly registry = new PaneRegistry();
  private readonly layout = new Layout();
  private readonly persister: StatePersister;
  private readonly lifecycle: PaneLifecycle;
  private readonly io: PaneIo;
  private readonly events: EventLog;
  private readonly conns = new Set<Connection>();
  private server: net.Server | undefined;
  private readonly startedAt = Date.now();
  /** 起動ごとに変わる ID。seq の巻き戻り (デーモン再起動) をクライアントが検知するためのもの。 */
  private readonly epoch = ulid();
  private readonly handlers: Handlers;

  constructor(opts: DaemonOptions) {
    const limits = resolveDaemonLimits(opts);
    this.events = new EventLog(limits.events);
    this.version = opts.version;
    this.socketPath = opts.socketPath;
    this.pidPath = opts.pidPath;
    this.statePath = opts.statePath;
    const sink = opts.log ?? ((m) => process.stderr.write(`[misao ${nowIso()}] ${m}\n`));
    this.log = createLogger(opts.logLevel ?? DEFAULT_LOG_LEVEL, sink);
    const save = opts.saveState ?? savePersistedState;
    this.persister = new StatePersister({
      snapshot: () => ({ version: 1, workspaces: this.layout.toPersisted(), panes: this.registry.toPersisted() }),
      save: (state) => save(this.statePath, state),
      log: this.log,
    });
    this.lifecycle = new PaneLifecycle({
      socketPath: this.socketPath,
      layout: this.layout,
      registry: this.registry,
      events: this.events,
      persister: this.persister,
      profiles: opts.profiles ?? [],
      log: this.log,
      limits,
      releasePane: (id) => this.io.release(id),
    });
    this.io = new PaneIo({
      registry: this.registry,
      events: this.events,
      persister: this.persister,
      conns: this.conns,
      epoch: this.epoch,
    });
    this.handlers = {
      'server.info': () => this.serverInfo(),
      'server.schema': () => buildProtocolJsonSchema(),
      'pane.open': (p) => this.lifecycle.open(p),
      'pane.info': (p) => this.info(p.paneId),
      'pane.list': (p) => this.paneList(p),
      'pane.write': (p) => this.io.write(p),
      'pane.resize': (p) => this.io.resize(p),
      'pane.screen': (p) => this.io.livePane(p.paneId).screen(),
      'pane.attach': (p, ctx) => this.io.attach(p, ctx),
      'pane.detach': (p, ctx) => this.io.detachPane(p, ctx),
      'pane.subscribe_lines': (p, ctx) => this.io.subscribeLines(p, ctx),
      'events.subscribe': (p, ctx) => this.subscribeEvents(p, ctx),
      'pane.set_label': (p) => this.lifecycle.setLabel(p),
      'pane.close': async (p) => {
        await this.lifecycle.close(p.paneId);
        return { ok: true };
      },
      ...createLayoutHandlers({
        layout: this.layout,
        events: this.events,
        closePane: (id) => this.lifecycle.close(id),
        paneIdsIn: (windowIds) => this.registry.filter(undefined, undefined, windowIds).map((e) => e.record.paneId),
        persist: () => this.persister.saveNow(),
      }),
    };
  }

  async start(): Promise<void> {
    await ensureSocketDir(this.socketPath);
    acquirePidFile(this.pidPath, process.pid);
    // pid ファイルで排他を取った後は、socketPath にあるファイルを自分のものとして扱える。
    try {
      this.loadState();
      if (await removeStaleSocket(this.socketPath)) this.log.info('removed stale socket');
    } catch (e) {
      fs.rmSync(this.pidPath, { force: true });
      throw e;
    }
    try {
      this.server = net.createServer((socket) => this.accept(socket));
      await listenUnixSocket(this.server, this.socketPath);
      // listen 後の accept 失敗 (EMFILE など) でデーモンごと落ちないよう、その接続だけを諦める。
      this.server.on('error', (e) => this.log.error(`server error: ${e.message}`));
    } catch (e) {
      this.server?.close();
      for (const f of [this.socketPath, this.pidPath]) fs.rmSync(f, { force: true });
      throw e;
    }
    this.events.emit('daemon.started', { pid: process.pid, protocolVersion: PROTOCOL_VERSION });
    this.log.info(`listening on ${this.socketPath} (pid ${process.pid})`);
  }

  async shutdown(): Promise<void> {
    this.log.info('shutting down');
    // close() は接続が全部閉じるまで完了しないので、先に接続を切ってから待つ。
    const server = this.server;
    const closed = new Promise<void>((resolve) => (server ? server.close(() => resolve()) : resolve()));
    for (const conn of this.conns) conn.socket.destroy();
    await closed;
    // record は消さない (再起動後に stopped として復元される)。遅らせていたサイズの保存だけを済ませる。
    this.persister.flush();
    const live = this.registry.livePanes();
    await Promise.all(live.map((p) => p.close()));
    for (const p of live) {
      this.io.release(p.id);
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
    for (const paneId of [...conn.attachments.keys()]) this.io.detach(conn, paneId);
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
        this.log.error(`internal error in ${req.method}: ${(e as Error).stack}`);
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
      version: this.version,
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

  private info(paneId: string): PaneInfo {
    const { workspace, window } = this.layout.windowRef(this.registry.getOrThrow(paneId).record.windowId);
    return this.registry.info(paneId, workspace, window);
  }

  private paneList(p: ParsedParams<'pane.list'>): PaneInfo[] {
    const { state, labels, workspace } = p.filter ?? {};
    const windowIds = workspace === undefined ? undefined : new Set(this.layout.windowIds(workspace));
    return this.registry.filter(state, labels, windowIds).map((e) => this.info(e.record.paneId));
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
}
