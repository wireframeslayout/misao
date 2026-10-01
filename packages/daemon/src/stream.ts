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
  /** ring に新しい要素が入ったときの通知を購読する。戻り値で解除。 */
  on: (cb: () => void) => () => void;
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

/**
 * ring を購読ごとのカーソルで読んで送る共通処理 (pull 型)。since 省略時はライブのみ。
 * 送信キューが溢れるまで先読みせず、drain に合わせて続きを送る。遅い購読者のぶんはリングが保持する。
 * リングに追い越された (カーソルの次が追い出された) 購読者だけ切断する。再接続後に gap で知らされる。
 */
export function subscribeStream<T>(opts: StreamOptions<T>): StreamSubscription {
  const { ctx, key, ring, notify, currentEpoch } = opts;
  const { conn } = ctx;
  const epochMismatch = opts.since !== undefined && opts.epoch !== undefined && opts.epoch !== currentEpoch;
  const since = epochMismatch ? 0 : opts.since;
  const head = ring.head;
  const gap = epochMismatch || (since !== undefined && ring.hasGap(since));
  // 最後に送った seq。gap のときは保持している最古から、head より先 (seq 巻き戻り) なら head から送る。
  let cursor = since === undefined ? head : Math.min(Math.max(since, ring.oldest - 1), head);
  let active = false;

  const pump = (): void => {
    if (!active) return; // 応答より先に通知しない
    if (cursor + 1 < ring.oldest) {
      conn.socket.destroy(); // 追い越された。書けない状態で到着しても判定する (読まないクライアントの検出)
      return;
    }
    while (conn.isStreamWritable()) {
      const entry = ring.entry(cursor + 1);
      if (!entry) return;
      cursor = entry.seq;
      notify(entry.seq, entry.item, entry.ts);
    }
  };

  const offRing = opts.on(pump);
  const offDrain = conn.onDrain(pump);
  const off = (): void => {
    active = false;
    offRing();
    offDrain();
  };
  conn.subscriptions.get(key)?.(); // 同じ (stream, pane) の既存購読は置き換える
  conn.subscriptions.set(key, off);
  ctx.afterReply(() => {
    if (conn.subscriptions.get(key) !== off) return; // 直後の再購読で置き換え済み
    active = true;
    pump();
  });
  return { gap, head, epoch: currentEpoch };
}
