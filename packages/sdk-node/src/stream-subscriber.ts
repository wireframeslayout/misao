import type { MethodResult } from '@misao/protocol';
import { MisaoRpcError } from './errors.js';
import { Listeners } from './listeners.js';
import type { Notification, RpcConnection, Settled } from './rpc-connection.js';
import { EVENTS_KEY, StreamCursor, keyOf, linesKey, toStreamId } from './stream-cursor.js';
import type { EventHandler, LineHandler, StreamEntry, StreamId, StreamPosition } from './stream-cursor.js';

/**
 * since と epoch は組で渡す (Subscription.cursor の値をそのまま渡せる)。epoch が無いとデーモンは
 * 世代を比べられず、再起動をまたいだ seq を気づかないまま使ってしまうため、片方だけは型で禁じる。
 * 省略すると購読時点からのライブのみ。epoch が現在と違えばデーモンは最古から再生し、gap('epoch') を通知する。
 */
export type SubscribeOptions = { since?: undefined; epoch?: undefined } | { since: number; epoch: string };

export interface Subscription {
  /** 現在位置。読むたびに最新の値を返す。 */
  readonly cursor: StreamPosition;
  unsubscribe(): void;
}

/** 'epoch': デーモン再起動で seq が巻き戻った。 'truncated': since が保持範囲より古い (または先)。 */
export type GapReason = 'epoch' | 'truncated';

export interface GapInfo {
  stream: StreamId;
  reason: GapReason;
}

export interface SubscriptionErrorInfo {
  stream: StreamId;
  error: MisaoRpcError;
}

type SubscribeResult = MethodResult<'events.subscribe'>;

function sendSubscribe(
  conn: RpcConnection,
  stream: StreamId,
  params: { since?: number; epoch?: string },
  onSettled: (outcome: Settled<SubscribeResult>) => void,
): void {
  if (stream.kind === 'events') conn.requestSync('events.subscribe', params, onSettled);
  else conn.requestSync('pane.subscribe_lines', { paneId: stream.paneId, ...params }, onSettled);
}

/**
 * 購読の登録・再購読・通知の振り分けを担う。接続の寿命は知らず、呼び出し側が接続を渡す。
 *
 * 順序への依存: デーモンは購読応答を送った後 (afterReply) に再生通知を流す。
 * そのため応答の反映 (lastSeq の設定・gap の通知) は requestSync の同期 callback で行い、
 * 同じチャンクで続く再生通知より必ず先に処理する。
 */
export class StreamSubscriber {
  private readonly cursor = new StreamCursor();
  private readonly pendingKeys = new Set<string>();
  private readonly gapListeners: Listeners<GapInfo>;
  private readonly errorListeners: Listeners<SubscriptionErrorInfo>;

  /** report: リスナーと購読ハンドラが投げた例外の報告先。配信は例外で止まらない。 */
  constructor(private readonly report: (error: unknown) => void) {
    this.gapListeners = new Listeners(report);
    this.errorListeners = new Listeners(report);
  }

  onGap(callback: (gap: GapInfo) => void): () => void {
    return this.gapListeners.add(callback);
  }

  onSubscriptionError(callback: (info: SubscriptionErrorInfo) => void): () => void {
    return this.errorListeners.add(callback);
  }

  subscribeEvents(conn: RpcConnection, handler: EventHandler, options: SubscribeOptions): Promise<Subscription> {
    return this.subscribe(conn, { kind: 'events', lastSeq: options.since ?? 0, handler }, options);
  }

  subscribeLines(
    conn: RpcConnection,
    paneId: string,
    handler: LineHandler,
    options: SubscribeOptions,
  ): Promise<Subscription> {
    return this.subscribe(conn, { kind: 'lines', paneId, lastSeq: options.since ?? 0, handler }, options);
  }

  /** 接続ごとに呼ぶ。epoch が変わっていれば全ストリームを巻き戻して gap を通知し、全ストリームを再購読する。 */
  async restore(conn: RpcConnection, epoch: string): Promise<void> {
    // epoch の状態は先に確定するが、emit はリスナーの例外を外に出さないので全ストリームへの通知は漏れない。
    const reset = this.cursor.resetForEpoch(epoch);
    for (const entry of reset) this.gapListeners.emit({ stream: toStreamId(entry), reason: 'epoch' });
    await Promise.all(this.cursor.list().map((entry) => this.resubscribe(conn, entry, reset.includes(entry))));
  }

  /** event / pane.line を購読先へ配る。ストリーム通知なら true (購読解除後の残りも含めて消費する)。 */
  dispatch(notification: Notification): boolean {
    if (notification.method === 'event') {
      const entry = this.cursor.get(EVENTS_KEY);
      if (entry?.kind === 'events' && this.cursor.accept(EVENTS_KEY, notification.params.seq)) {
        this.deliver(() => entry.handler(notification.params));
      }
      return true;
    }
    if (notification.method === 'pane.line') {
      const key = linesKey(notification.params.paneId);
      const entry = this.cursor.get(key);
      if (entry?.kind === 'lines' && this.cursor.accept(key, notification.params.seq)) {
        this.deliver(() => entry.handler(notification.params));
      }
      return true;
    }
    return false;
  }

  /** ハンドラの例外は報告して続ける。同じチャンクの後続行の配信と lastSeq の進行を止めない。 */
  private deliver(call: () => void): void {
    try {
      call();
    } catch (error) {
      this.report(error);
    }
  }

  private async subscribe(conn: RpcConnection, entry: StreamEntry, options: SubscribeOptions): Promise<Subscription> {
    const key = keyOf(entry);
    if (this.cursor.has(key) || this.pendingKeys.has(key)) throw new Error(`stream already registered: ${key}`);
    this.pendingKeys.add(key);
    try {
      await new Promise<void>((resolve, reject) => {
        sendSubscribe(conn, entry, { since: options.since, epoch: options.epoch }, (outcome) => {
          if (!outcome.ok) {
            reject(outcome.error);
            return;
          }
          this.activate(entry, options, outcome.value);
          resolve();
        });
      });
    } finally {
      this.pendingKeys.delete(key);
    }
    const streamCursor = this.cursor;
    return {
      get cursor(): StreamPosition {
        return streamCursor.position(entry);
      },
      unsubscribe: () => this.cursor.remove(entry),
    };
  }

  private activate(entry: StreamEntry, options: SubscribeOptions, result: SubscribeResult): void {
    // since なし (ライブのみ) は head から。since ありなら epoch が現在と違うかを比べる。
    const epochChanged = options.since !== undefined && options.epoch !== result.epoch;
    // epoch が違うと最古から再生されるので、古い since は引き継がない。
    // since が head より先なら head に寄せる (先の seq を重複扱いで捨てないため)。
    entry.lastSeq = epochChanged ? 0 : Math.min(options.since ?? result.head, result.head);
    this.cursor.add(entry);
    if (result.gap) this.gapListeners.emit({ stream: toStreamId(entry), reason: epochChanged ? 'epoch' : 'truncated' });
  }

  private resubscribe(conn: RpcConnection, entry: StreamEntry, isEpochReset: boolean): Promise<void> {
    return new Promise((resolve, reject) => {
      sendSubscribe(conn, entry, this.cursor.resubscribeParams(entry), (outcome) => {
        if (outcome.ok) {
          // epoch 変化は restore の冒頭で通知済み。同じストリームに二重に gap を出さない。
          if (outcome.value.gap && !isEpochReset) this.gapListeners.emit({ stream: toStreamId(entry), reason: 'truncated' });
          resolve();
        } else if (outcome.error instanceof MisaoRpcError) {
          const isActive = this.cursor.get(keyOf(entry)) === entry;
          this.cursor.remove(entry);
          // 応答待ちの間に unsubscribe されたストリームは利用側の関心外なので知らせない。
          if (isActive) this.errorListeners.emit({ stream: toStreamId(entry), error: outcome.error });
          resolve();
        } else {
          reject(outcome.error);
        }
      });
    });
  }
}
