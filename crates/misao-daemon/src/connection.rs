//! 1 接続ぶんの入出力。ストリームの種類（Unix ソケット・名前付きパイプ）には依存しない。
//!
//! 送信は接続ごとの送信キュー（無制限の mpsc）+ 書き込みタスクで行い、キューに溜まったバイト数を数える。
//! 応答はキューが `MAX_WRITABLE_BYTES` を超えたら接続ごと切る（読まないクライアントの検出）。
//! 購読ストリーム（行・イベント）は `STREAM_HIGH_WATER_BYTES` 未満の間だけ送り、超えたら `drain` を待つ。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use misao_protocol::{LineSplitter, Response, encode_message};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::attachment::Attachment;
use crate::core::Core;
use crate::rpc;

/// 応答などでこれ以上送信キューが溜まった接続（遅い consumer）は切る。行・イベントの購読は pull 型でこの上限を使わない。
pub const MAX_WRITABLE_BYTES: usize = 16 * 1024 * 1024;

/// 購読ストリームはキューがこれ未満の間だけ送る。超えたら drain を待つ（送信キューを小さく保つ）。
pub const STREAM_HIGH_WATER_BYTES: usize = 1024 * 1024;

/// Parse error を返して切るとき、相手が読まなくても残さないための待ち時間の上限。
const REJECT_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ConnHandle {
    tx: Mutex<Option<UnboundedSender<Vec<u8>>>>,
    queued: AtomicUsize,
    closed: AtomicBool,
    /// 書き込みタスクがキューを 1 件書き終えるたびに進む。購読ストリームの drain 待ち。
    drain_tx: watch::Sender<u64>,
    kill_tx: watch::Sender<bool>,
    /// `${stream}:${paneId}` → 購読タスク。同じキーの再購読は既存を置き換える（二重配信の防止）。
    subscriptions: Mutex<HashMap<String, AbortHandle>>,
    /// paneId → この接続の attach。1 つの pane につき 1 つ（同じ clientId の再 attach は置き換える）。
    attachments: Mutex<HashMap<String, Arc<Attachment>>>,
}

impl ConnHandle {
    pub(crate) fn new() -> (Arc<Self>, UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = unbounded_channel();
        let conn = Arc::new(Self {
            tx: Mutex::new(Some(tx)),
            queued: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            drain_tx: watch::Sender::new(0),
            kill_tx: watch::Sender::new(false),
            subscriptions: Mutex::new(HashMap::new()),
            attachments: Mutex::new(HashMap::new()),
        });
        (conn, rx)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 1 メッセージ（NDJSON の 1 行）を送信キューへ入れる。閉じた接続には何もしない。
    pub fn send<T: Serialize>(&self, msg: &T) {
        if let Ok(bytes) = encode_message(msg) {
            self.enqueue(bytes);
        }
    }

    pub fn send_response(&self, response: &Response) {
        self.send(response);
    }

    /// 組み立て済みの 1 行（NDJSON）を送信キューへ入れる。
    pub fn send_bytes(&self, bytes: Vec<u8>) {
        self.enqueue(bytes);
    }

    fn enqueue(&self, bytes: Vec<u8>) {
        if self.is_closed() {
            return;
        }
        let len = bytes.len();
        // 溜まっているバイト数に、このメッセージのぶんを原子的に足して検査する。1 つのメッセージで上限を超える場合も
        // 投入せずに接続を切る（読まないクライアントの検出と、大きな 1 通で上限を飛び越えることの防止）。
        let mut queued = self.queued.load(Ordering::Acquire);
        let reserved = loop {
            let Some(total) = queued
                .checked_add(len)
                .filter(|total| *total <= MAX_WRITABLE_BYTES)
            else {
                break false;
            };
            match self
                .queued
                .compare_exchange_weak(queued, total, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break true,
                Err(current) => queued = current,
            }
        };
        if !reserved {
            self.destroy();
            return;
        }
        let guard = self.tx.lock().unwrap_or_else(PoisonError::into_inner);
        let sent = guard.as_ref().is_some_and(|tx| tx.send(bytes).is_ok());
        if !sent {
            self.queued.fetch_sub(len, Ordering::AcqRel);
        }
    }

    /// 購読ストリームが続きを送ってよいか。接続が生きていて、送信キューが `STREAM_HIGH_WATER_BYTES` 未満のとき。
    pub fn is_stream_writable(&self) -> bool {
        !self.is_closed() && self.queued.load(Ordering::Acquire) < STREAM_HIGH_WATER_BYTES
    }

    /// 送信キューが書き進んだときの通知。
    pub fn drain(&self) -> watch::Receiver<u64> {
        self.drain_tx.subscribe()
    }

    /// 接続を即座に切る（未送信のものは捨てる）。
    pub fn destroy(&self) {
        self.closed.store(true, Ordering::Release);
        self.kill_tx.send_replace(true);
        self.abort_subscriptions();
    }

    /// 送信キューを書き切ってから閉じる（これ以上は送れない）。
    fn finish(&self) {
        self.tx.lock().unwrap_or_else(PoisonError::into_inner).take();
    }

    fn abort_subscriptions(&self) {
        let mut subs = self.subscriptions.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, handle) in subs.drain() {
            handle.abort();
        }
    }

    pub fn attachment(&self, pane_id: &str) -> Option<Arc<Attachment>> {
        self.attachments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(pane_id)
            .cloned()
    }

    /// 登録して、同じ pane の古い attach を返す。
    pub fn replace_attachment(&self, pane_id: &str, attachment: Arc<Attachment>) -> Option<Arc<Attachment>> {
        self.attachments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pane_id.to_string(), attachment)
    }

    pub fn take_attachment(&self, pane_id: &str) -> Option<Arc<Attachment>> {
        self.attachments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(pane_id)
    }

    /// 指定の attach が今の attach のままなら外す（pane の close で、後から置き換わったものを消さない）。
    pub fn remove_attachment_if(&self, pane_id: &str, attachment_id: u64) -> bool {
        let mut attachments = self.attachments.lock().unwrap_or_else(PoisonError::into_inner);
        if attachments.get(pane_id).is_some_and(|a| a.id == attachment_id) {
            attachments.remove(pane_id);
            return true;
        }
        false
    }

    pub fn attached_pane_ids(&self) -> Vec<String> {
        self.attachments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// 同じキーの既存の購読を止める。
    pub fn cancel_subscription(&self, key: &str) {
        let removed = self
            .subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
        if let Some(handle) = removed {
            handle.abort();
        }
    }

    /// 購読タスクを登録する。接続が既に閉じていれば止める。
    /// `done` は購読タスクが終わる直前に立てる印（先に終わっていたら登録しない）。
    pub fn register_subscription(&self, key: String, handle: AbortHandle, done: &AtomicBool) {
        let mut subs = self.subscriptions.lock().unwrap_or_else(PoisonError::into_inner);
        if self.is_closed() {
            handle.abort();
            return;
        }
        if done.load(Ordering::Acquire) {
            return;
        }
        if let Some(old) = subs.insert(key, handle) {
            old.abort();
        }
    }

    /// 購読タスクが自分で終わったとき、自分の登録を外す（pane の close などで終わった購読を残さない）。
    /// 後から同じキーで置き換えられていれば、そちらは外さない。
    pub fn forget_subscription(&self, key: &str, task: tokio::task::Id) {
        let mut subs = self.subscriptions.lock().unwrap_or_else(PoisonError::into_inner);
        if subs.get(key).is_some_and(|handle| handle.id() == task) {
            subs.remove(key);
        }
    }
}

/// 接続のタスクが終わる（正常・abort のどちらでも）と接続を片付ける。attach していた pane からも外す。
struct ConnGuard {
    conn: Arc<ConnHandle>,
    core: Arc<Core>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.conn.destroy();
        self.core.connection_closed(&self.conn);
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: UnboundedReceiver<Vec<u8>>,
    conn: Arc<ConnHandle>,
) {
    let mut killed = conn.kill_tx.subscribe();
    loop {
        let bytes = tokio::select! {
            next = rx.recv() => match next {
                Some(bytes) => bytes,
                None => break,
            },
            _ = killed.wait_for(|k| *k) => return,
        };
        let len = bytes.len();
        let written = tokio::select! {
            r = writer.write_all(&bytes) => r,
            _ = killed.wait_for(|k| *k) => return,
        };
        if written.is_err() {
            conn.destroy();
            return;
        }
        conn.queued.fetch_sub(len, Ordering::AcqRel);
        conn.drain_tx.send_modify(|n| *n = n.wrapping_add(1));
    }
    let _ = writer.shutdown().await;
}

/// 接続を最後まで処理する。クライアントが閉じるか、サーバー側で切るか、タスクが abort されると終わる。
pub async fn serve_connection<S>(stream: S, core: Arc<Core>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, writer) = tokio::io::split(stream);
    let (conn, rx) = ConnHandle::new();
    let _guard = ConnGuard {
        conn: Arc::clone(&conn),
        core: Arc::clone(&core),
    };
    let mut writer_task = tokio::spawn(write_loop(writer, rx, Arc::clone(&conn)));
    let mut killed = conn.kill_tx.subscribe();
    let mut splitter = LineSplitter::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = tokio::select! {
            read = reader.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            },
            _ = killed.wait_for(|k| *k) => break,
        };
        let lines = match splitter.push(&buf[..n]) {
            Ok(lines) => lines,
            Err(e) => {
                // 行の上限超過など、続行できない入力。Parse error を返して切る。
                conn.send_response(&Response::parse_error(e.to_string()));
                conn.finish();
                let _ = tokio::time::timeout(REJECT_FLUSH_TIMEOUT, &mut writer_task).await;
                break;
            }
        };
        for line in lines {
            if conn.is_closed() {
                break;
            }
            rpc::handle_line(&core, &conn, &line).await;
            // まとめて届いたリクエストの処理中も、切断・シャットダウンに応じられるようにする。
            tokio::task::yield_now().await;
        }
    }
    writer_task.abort();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_message_that_would_exceed_the_queue_limit_is_not_queued_and_drops_the_connection() {
        let (conn, _rx) = ConnHandle::new();
        conn.send_bytes(vec![0; MAX_WRITABLE_BYTES - 10]);
        assert!(!conn.is_closed());
        conn.send_bytes(vec![0; 11]);
        assert!(conn.is_closed(), "the 11 bytes that cross the limit are refused");
        assert_eq!(
            conn.queued.load(Ordering::Acquire),
            MAX_WRITABLE_BYTES - 10,
            "nothing over the limit was reserved"
        );
    }

    #[tokio::test]
    async fn a_subscription_that_ends_by_itself_is_forgotten_and_a_replacement_is_kept() {
        let (conn, _rx) = ConnHandle::new();
        let done = AtomicBool::new(false);
        let first = tokio::spawn(std::future::pending::<()>());
        conn.register_subscription("lines:p".into(), first.abort_handle(), &done);
        let second = tokio::spawn(std::future::pending::<()>());
        conn.register_subscription("lines:p".into(), second.abort_handle(), &done);
        conn.forget_subscription("lines:p", first.id()); // 置き換えられた古い購読の終了は、新しい購読を外さない
        assert_eq!(conn.subscriptions.lock().unwrap().len(), 1);
        conn.forget_subscription("lines:p", second.id());
        assert!(conn.subscriptions.lock().unwrap().is_empty());
        // 登録より先に終わっていた購読は登録しない
        let third = tokio::spawn(std::future::pending::<()>());
        conn.register_subscription("lines:q".into(), third.abort_handle(), &AtomicBool::new(true));
        assert!(conn.subscriptions.lock().unwrap().is_empty());
    }

    #[test]
    fn one_huge_message_on_an_empty_queue_drops_the_connection() {
        let (conn, _rx) = ConnHandle::new();
        conn.send_bytes(vec![0; MAX_WRITABLE_BYTES + 1]);
        assert!(conn.is_closed());
        assert_eq!(conn.queued.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_pending_attachment_that_buffers_too_much_before_the_reply_drops_the_connection() {
        let (conn, _rx) = ConnHandle::new();
        let attachment = Attachment::new("p_x", "A", &conn, 0);
        let chunk = vec![0u8; 1024 * 1024];
        for seq in 1..=20u64 {
            attachment.deliver(seq, "t", &chunk, &[]);
        }
        assert!(conn.is_closed(), "16 MiB waiting for the reply is the limit");
        attachment.go_live(); // 切った後は何も送らない
    }
}
