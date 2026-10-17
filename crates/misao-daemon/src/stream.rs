//! リングを購読ごとのカーソルで読んで送る共通処理（pull 型。Node 版の `subscribeStream`）。
//!
//! `since` 省略時はライブのみ。送信キューが溢れるまで先読みせず、drain に合わせて続きを送る。
//! 遅い購読者のぶんはリングが保持する。リングに追い越された（カーソルの次が追い出された）購読者だけ切断し、
//! 再接続後に gap で知らされる。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::watch;

use crate::connection::ConnHandle;
use crate::rpc::RequestContext;

/// 1 回に連続して送る件数の上限。これを超えたら他のタスクに実行を譲る。
const YIELD_EVERY: u32 = 256;

/// 購読者がこの時間カーソルを進めず、かつ追いついてもいないなら、止まっている（読んでいない）とみなす。
pub const STALL_AFTER: Duration = Duration::from_secs(1);

/// 購読者の進み具合。送り手（pty を読むスレッド）が、遅れている購読者を待つかどうかの判断に使う。
/// 購読のタスクが終わると（`Arc` が落ちると）送り手の側からは見えなくなる。
#[derive(Debug)]
pub struct Progress {
    cursor: AtomicU64,
    /// 最後に進んだ（または追いついていた）時刻。`started` からの経過ミリ秒。
    active_ms: AtomicU64,
    started: Instant,
}

impl Progress {
    fn new(cursor: u64) -> Self {
        Self {
            cursor: AtomicU64::new(cursor),
            active_ms: AtomicU64::new(0),
            started: Instant::now(),
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// カーソルが進んだ。
    fn advance(&self, cursor: u64) {
        self.cursor.store(cursor, Ordering::Release);
        self.active_ms.store(self.elapsed_ms(), Ordering::Release);
    }

    /// 追いついている（送るものが無い）。止まっているわけではない。
    fn caught_up(&self) {
        self.active_ms.store(self.elapsed_ms(), Ordering::Release);
    }

    /// 最後に送った seq。
    pub fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Acquire)
    }

    pub fn is_stalled(&self) -> bool {
        let idle = self
            .elapsed_ms()
            .saturating_sub(self.active_ms.load(Ordering::Acquire));
        Duration::from_millis(idle) >= STALL_AFTER
    }
}

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
    /// 購読者の進み具合を受け取る。送り手が購読者を待つ必要がある購読元だけが実装する。
    fn track(&self, _progress: &Arc<Progress>) {}
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
    let gap = epoch_mismatch || since.is_some_and(|s| has_gap(s, head, oldest));
    // 最後に送った seq。gap のときは保持している最古から、head より先（seq 巻き戻り）なら head から送る。
    let cursor = since.map_or(head, |s| s.max(oldest - 1).min(head));
    let conn = Arc::clone(&ctx.conn);
    conn.cancel_subscription(&key); // 同じ (stream, pane) の既存購読は置き換える
    let progress = Arc::new(Progress::new(cursor));
    source.track(&progress);
    ctx.after_reply(move || {
        // 応答より先に通知しない。
        let handle = tokio::spawn(pump(source, Arc::clone(&conn), progress, wake));
        conn.register_subscription(key, handle.abort_handle());
    });
    json!({"gap": gap, "head": head, "epoch": current_epoch})
}

/// since が保持範囲より古い、または head より先（= 送信側が再起動して seq が巻き戻った）なら欠落扱い。
fn has_gap(since: u64, head: u64, oldest: u64) -> bool {
    since + 1 < oldest || since > head
}

async fn pump(
    source: Arc<dyn StreamSource>,
    conn: Arc<ConnHandle>,
    progress: Arc<Progress>,
    mut wake: watch::Receiver<u64>,
) {
    let mut cursor = progress.cursor();
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
                    progress.advance(cursor);
                    conn.send(&notification);
                    sent_in_a_row += 1;
                    if sent_in_a_row >= YIELD_EVERY {
                        sent_in_a_row = 0;
                        tokio::task::yield_now().await;
                    }
                }
                Next::Empty => {
                    progress.caught_up();
                    break;
                }
                Next::Blocked => break,
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

#[cfg(test)]
mod tests {
    use super::has_gap;

    #[test]
    fn an_empty_stream_has_oldest_one_past_head() {
        assert!(!has_gap(0, 0, 1));
        assert!(has_gap(1, 0, 1), "since ahead of head is a gap");
    }

    #[test]
    fn since_just_before_the_oldest_is_still_contiguous() {
        // 保持範囲は 3..=5
        assert!(!has_gap(2, 5, 3));
        assert!(has_gap(1, 5, 3), "evicted entries are missing");
        assert!(!has_gap(5, 5, 3), "since = head: nothing missed");
        assert!(has_gap(6, 5, 3), "seq was rewound");
    }
}
