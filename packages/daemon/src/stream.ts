import type { Connection } from './connection.js';
import type { SeqRing } from './ring.js';

export interface RequestContext {
  conn: Connection;
  /** 応答を送った後に実行する。 */
  afterReply(fn: () => void): void;
}

export interface StreamOptions<T> {
  ctx: RequestContext;
  /** `${stream}:${paneId}` など。同じキーの再購読は既存を置き換える。 */
  key: string;
  ring: SeqRing<T>;
  on: (cb: (seq: number, item: T, ts: string) => void) => () => void;
  notify: (seq: number, item: T, ts: string) => void;
  since: number | undefined;
  /** since が属する epoch。現在と違えば seq が巻き戻っているので、保持している最古から再生する。 */
  epoch: string | undefined;
  currentEpoch: string;
}

export interface StreamSubscription {
  gap: boolean;
  head: number;
  epoch: string;
}

/** ring の since 以降を再生してからライブに切り替える共通処理。since 省略時はライブのみ。 */
export function subscribeStream<T>(opts: StreamOptions<T>): StreamSubscription {
  const { ctx, key, ring, notify, currentEpoch } = opts;
  const epochMismatch = opts.since !== undefined && opts.epoch !== undefined && opts.epoch !== currentEpoch;
  const since = epochMismatch ? 0 : opts.since;
  const head = ring.head;
  const gap = epochMismatch || (since !== undefined && ring.hasGap(since));
  const queue: Array<[number, T, string]> = [];
  let live = false;
  const off = opts.on((seq, item, ts) => (live ? notify(seq, item, ts) : queue.push([seq, item, ts])));
  ctx.conn.subscriptions.get(key)?.(); // 同じ (stream, pane) の既存購読は置き換える
  ctx.conn.subscriptions.set(key, off);
  ctx.afterReply(() => {
    if (ctx.conn.subscriptions.get(key) !== off) return; // 直後の再購読で置き換え済み
    if (since !== undefined) for (const e of ring.since(since)) if (e.seq <= head) notify(e.seq, e.item, e.ts);
    for (const [seq, item, ts] of queue) if (seq > head) notify(seq, item, ts);
    queue.length = 0;
    live = true;
  });
  return { gap, head, epoch: currentEpoch };
}
