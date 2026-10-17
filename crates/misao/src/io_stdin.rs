//! 標準入力(チャンクの列として受け取り、行の読み取りでは余りを次に持ち越す)。

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc::UnboundedReceiver;
#[cfg(all(test, unix))]
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::RawMode;

pub(super) type Starter = Box<dyn FnOnce() -> UnboundedReceiver<Vec<u8>> + Send>;

pub struct Stdin {
    pub is_tty: bool,
    start: Mutex<Option<Starter>>,
    rx: tokio::sync::Mutex<Option<UnboundedReceiver<Vec<u8>>>>,
    pending: Mutex<Vec<u8>>,
    raw: Arc<dyn RawMode>,
}

impl Stdin {
    pub fn new(is_tty: bool, raw: Arc<dyn RawMode>, start: Starter) -> Self {
        Self {
            is_tty,
            start: Mutex::new(Some(start)),
            rx: tokio::sync::Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            raw,
        }
    }

    /// テスト用: 送り口から入力を流せる標準入力。送り口を落とすと EOF。
    #[cfg(all(test, unix))]
    pub fn channel(is_tty: bool, raw: Arc<dyn RawMode>) -> (Self, UnboundedSender<Vec<u8>>) {
        let (tx, rx) = unbounded_channel();
        let stdin = Self::new(is_tty, raw, Box::new(move || rx));
        (stdin, tx)
    }

    pub fn set_raw(&self, raw: bool) -> std::io::Result<()> {
        self.raw.set_raw(raw)
    }

    fn take_pending(&self) -> Vec<u8> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// 次のチャンク。持ち越しがあれば先にそれを返す。入力が閉じたら None。
    pub async fn recv(&self) -> Option<Vec<u8>> {
        let pending = self.take_pending();
        if !pending.is_empty() {
            return Some(pending);
        }
        self.recv_chunk().await
    }

    async fn recv_chunk(&self) -> Option<Vec<u8>> {
        let mut rx = self.rx.lock().await;
        if rx.is_none() {
            let start = self.start.lock().unwrap_or_else(PoisonError::into_inner).take();
            *rx = start.map(|f| f());
        }
        rx.as_mut()?.recv().await
    }

    /// 1 行読む（改行は除く）。入力が閉じたとき、改行なしの残りがあればそれを、無ければ None。
    pub async fn read_line(&self) -> Option<String> {
        loop {
            {
                let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(at) = pending.iter().position(|&b| b == b'\n') {
                    let rest = pending.split_off(at + 1);
                    let line = std::mem::replace(&mut *pending, rest);
                    let text = String::from_utf8_lossy(&line[..at]).into_owned();
                    return Some(text.strip_suffix('\r').map_or(text.clone(), str::to_string));
                }
            }
            match self.recv_chunk().await {
                Some(chunk) => self
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(&chunk),
                None => {
                    let rest = self.take_pending();
                    return (!rest.is_empty()).then(|| String::from_utf8_lossy(&rest).into_owned());
                }
            }
        }
    }

    /// 入力が閉じるまで全部読む。
    pub async fn read_all(&self) -> Vec<u8> {
        let mut all = self.take_pending();
        while let Some(chunk) = self.recv_chunk().await {
            all.extend_from_slice(&chunk);
        }
        all
    }
}
