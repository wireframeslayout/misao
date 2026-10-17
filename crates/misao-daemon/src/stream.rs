//! リングを購読ごとのカーソルで読んで送る共通処理（pull 型。Node 版の `subscribeStream`）。
//!
//! `since` 省略時はライブのみ。送信キューが溢れるまで先読みせず、drain に合わせて続きを送る。
//! 遅い購読者のぶんはリングが保持する。リングに追い越された（カーソルの次が追い出された）購読者だけ切断し、
//! 再接続後に gap で知らされる。

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::watch;

use crate::connection::ConnHandle;
use crate::rpc::RequestContext;

/// 1 回に連続して送る件数の上限。これを超えたら他のタスクに実行を譲る。
const YIELD_EVERY: u32 = 256;

pub enum Next {
    /// 次のエントリ。`notification` は送るメッセージ（JSON-RPC の通知）。
    Entry { seq: u64, notification: Value },
    /// 新しいエントリがまだ無い。
    Empty,
    /// 送信キューが一杯で、今は送れない。
    Blocked,
    /// カーソルの次が追い出された。
    Overtaken,
    /// 購読元が無くなった（pane の close など）。
    Released,
}

/// 購読できる seq 付きのリング。
pub trait StreamSource: Send + Sync + 'static {
    /// `(head, oldest)`。
    fn bounds(&self) -> (u64, u64);
    /// カーソル（最後に送った seq）の次を読む。`writable` が偽なら、読める状態でも `Blocked` を返す。
    fn next(&self, cursor: u64, writable: bool) -> Next;
    /// 新着の通知。
    fn wake(&self) -> watch::Receiver<u64>;
}

/// 購読の結果（`gap` / `head` / `epoch`）。
pub fn subscribe(
    ctx: &mut RequestContext,
    key: String,
    source: Arc<dyn StreamSource>,
    since: Option<u64>,
    epoch: Option<&str>,
    current_epoch: &str,
) -> Value {
    // since が属する epoch が今と違えば seq が巻き戻っているので、保持している最古から再生する。
    let epoch_mismatch = since.is_some() && epoch.is_some_and(|e| e != current_epoch);
    let since = if epoch_mismatch { Some(0) } else { since };
    // 新着の取りこぼしがないよう、境界を読む前に待ち受けを作る。
    let wake = source.wake();
    let (head, oldest) = source.bounds();
    let gap = epoch_mismatch || since.is_some_and(|s| s + 1 < oldest || s > head);
    // 最後に送った seq。gap のときは保持している最古から、head より先（seq 巻き戻り）なら head から送る。
    let cursor = since.map_or(head, |s| s.max(oldest - 1).min(head));
    let conn = Arc::clone(&ctx.conn);
    conn.cancel_subscription(&key); // 同じ (stream, pane) の既存購読は置き換える
    ctx.after_reply(move || {
        // 応答より先に通知しない。
        let handle = tokio::spawn(pump(source, Arc::clone(&conn), cursor, wake));
        conn.register_subscription(key, handle.abort_handle());
    });
    json!({"gap": gap, "head": head, "epoch": current_epoch})
}

async fn pump(
    source: Arc<dyn StreamSource>,
    conn: Arc<ConnHandle>,
    mut cursor: u64,
    mut wake: watch::Receiver<u64>,
) {
    let mut drain = conn.drain();
    let mut sent_in_a_row = 0u32;
    loop {
        loop {
            if conn.is_closed() {
                return;
            }
            match source.next(cursor, conn.is_stream_writable()) {
                Next::Entry { seq, notification } => {
                    cursor = seq;
                    conn.send(&notification);
                    sent_in_a_row += 1;
                    if sent_in_a_row >= YIELD_EVERY {
                        sent_in_a_row = 0;
                        tokio::task::yield_now().await;
                    }
                }
                Next::Empty | Next::Blocked => break,
                Next::Overtaken => {
                    // 読まないクライアントの検出。切断し、再接続後の購読で gap として知らせる。
                    conn.destroy();
                    return;
                }
                Next::Released => return,
            }
        }
        sent_in_a_row = 0;
        tokio::select! {
            changed = wake.changed() => if changed.is_err() { return },
            changed = drain.changed() => if changed.is_err() { return },
        }
    }
}
