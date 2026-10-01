import { ErrorCode } from '@misao/protocol';
import { attachPane } from './attach.js';
import type { Connection } from './connection.js';
import type { EventLog } from './event-log.js';
import type { ParsedParams } from './params.js';
import type { Pane } from './pane.js';
import type { PaneRegistry } from './pane-registry.js';
import { RpcFailure } from './rpc-error.js';
import type { StatePersister } from './state-persister.js';
import { subscribeStream } from './stream.js';
import type { RequestContext } from './stream.js';

/** 接続ごとの購読キー。同じキーの再購読は既存を置き換える。 */
const linesKey = (paneId: string): string => `lines:${paneId}`;

/** Daemon から受け取る依存。PaneIo は Daemon 本体を知らない。 */
export interface PaneIoHost {
  registry: PaneRegistry;
  events: EventLog;
  persister: StatePersister;
  conns: ReadonlySet<Connection>;
  epoch: string;
}

/** 実行中の pane への入出力 (write / resize / attach / detach / 行購読)。 */
export class PaneIo {
  constructor(private readonly host: PaneIoHost) {}

  /** 実行体のある pane。stopped (再起動後の復元分) は PaneExited。 */
  livePane(id: string): Pane {
    const { live } = this.host.registry.getOrThrow(id);
    if (!live) throw new RpcFailure(ErrorCode.PaneExited, 'pane is stopped');
    return live;
  }

  /**
   * サイズ (cols / rows) は再起動後の表示用の付帯情報なので、保存は遅らせる。
   * 保存の失敗で入力・resize・attach を落とさない。
   */
  write(p: ParsedParams<'pane.write'>): unknown {
    const { events, persister } = this.host;
    const pane = this.livePane(p.paneId);
    if (pane.state === 'exited') throw new RpcFailure(ErrorCode.PaneExited, 'pane has exited');
    const data = p.dataB64 !== undefined ? Buffer.from(p.dataB64, 'base64') : Buffer.from(p.data, 'utf8');
    // 最後に操作したクライアントのサイズを優先する
    if (p.clientId && pane.claimSize(p.clientId)) {
      persister.saveSoon();
      events.emit('pane.resized', { cols: pane.cols, rows: pane.rows, clientId: p.clientId }, pane.id);
    }
    pane.write(data);
    events.emit('input', { source: p.source ?? 'hub', bytes: data.length }, pane.id); // 内容は記録しない
    return { ok: true };
  }

  resize(p: ParsedParams<'pane.resize'>): unknown {
    const pane = this.livePane(p.paneId);
    const clientId = p.clientId ?? null;
    pane.resize(p.cols, p.rows, clientId);
    this.host.persister.saveSoon();
    this.host.events.emit('pane.resized', { cols: p.cols, rows: p.rows, clientId }, pane.id);
    return { ok: true };
  }

  async attach(p: ParsedParams<'pane.attach'>, ctx: RequestContext): Promise<unknown> {
    const { events, persister } = this.host;
    if (p.mode === 'cells') throw new RpcFailure(ErrorCode.Unsupported, 'mode "cells" is not supported');
    const pane = this.livePane(p.paneId);
    const replacing = ctx.conn.attachments.get(pane.id)?.clientId === p.clientId;
    if (replacing) this.replaceAttachment(ctx.conn, pane.id);
    else this.detach(ctx.conn, pane.id);
    if (p.cols !== undefined && p.rows !== undefined) {
      pane.resize(p.cols, p.rows, p.clientId);
      persister.saveSoon();
      events.emit('pane.resized', { cols: p.cols, rows: p.rows, clientId: p.clientId }, pane.id);
    }
    // attachPane は最初の await までに購読と clients への登録を同期的に済ませる
    const attached = attachPane(ctx, pane, p.clientId, p.replay);
    if (!replacing) events.emit('client.attached', { clientId: p.clientId }, pane.id);
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

  detachPane(p: ParsedParams<'pane.detach'>, ctx: RequestContext): unknown {
    this.detach(ctx.conn, p.paneId);
    return { ok: true };
  }

  /** 接続の切断からも呼ばれるので、サイズの継承は遅らせて保存する (失敗しても RPC は落とさない)。 */
  detach(conn: Connection, paneId: string): void {
    const att = conn.attachments.get(paneId);
    if (!att) return;
    att.off();
    conn.attachments.delete(paneId);
    const pane = this.host.registry.get(paneId)?.live;
    // 同じ clientId が別接続にも残っていれば clients から外さない
    const stillAttached = [...this.host.conns].some((c) => c !== conn && c.attachments.get(paneId)?.clientId === att.clientId);
    if (pane && !stillAttached) {
      pane.clients.delete(att.clientId);
      const inherited = pane.forgetClient(att.clientId);
      if (inherited) {
        this.host.events.emit('pane.resized', { cols: pane.cols, rows: pane.rows, clientId: inherited }, paneId);
        this.host.persister.saveSoon();
      }
    }
    this.host.events.emit('client.detached', { clientId: att.clientId }, paneId);
  }

  subscribeLines(p: ParsedParams<'pane.subscribe_lines'>, ctx: RequestContext): unknown {
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
      currentEpoch: this.host.epoch,
    });
  }

  /** 全接続から、この pane の attachment と行購読を外す (閉じた Pane への参照を残さない)。 */
  release(paneId: string): void {
    const key = linesKey(paneId);
    for (const conn of this.host.conns) {
      this.detach(conn, paneId);
      conn.subscriptions.get(key)?.();
      conn.subscriptions.delete(key);
    }
  }
}
