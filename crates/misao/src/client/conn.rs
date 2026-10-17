//! 1 本の接続の読み書きと、保留中の要求の対応。

use std::collections::HashMap;

use misao_protocol::{LineSplitter, encode_message};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::oneshot;

use super::ClientError;
use super::cursor::{StreamId, StreamPos};
use super::transport::Stream;

pub(super) enum Pending {
    User(oneshot::Sender<Result<Value, ClientError>>),
    Subscribe {
        stream: StreamId,
        since: Option<StreamPos>,
        reply: oneshot::Sender<Result<(), ClientError>>,
    },
    Resub {
        stream: StreamId,
        was_reset: bool,
    },
    Info,
}

pub(super) struct Conn {
    pub(super) read: ReadHalf<Stream>,
    pub(super) write: WriteHalf<Stream>,
    pub(super) splitter: LineSplitter,
    pub(super) pending: HashMap<u64, Pending>,
    pub(super) next_id: u64,
    pub(super) buf: Vec<u8>,
}

impl Conn {
    pub(super) fn new(stream: Stream) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            read,
            write,
            splitter: LineSplitter::default(),
            pending: HashMap::new(),
            next_id: 1,
            buf: vec![0u8; 64 * 1024],
        }
    }

    pub(super) async fn send(
        &mut self,
        method: &str,
        params: Value,
        pending: Pending,
    ) -> Result<(), ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let bytes = encode_message(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .map_err(|e| ClientError::Other(e.to_string()))?;
        self.pending.insert(id, pending);
        self.write
            .write_all(&bytes)
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))
    }

    /// 次のまとまりの行。相手が閉じたら Err。
    pub(super) async fn read_lines(&mut self) -> Result<Vec<String>, ClientError> {
        loop {
            let n = self
                .read
                .read(&mut self.buf)
                .await
                .map_err(|e| ClientError::Connection(e.to_string()))?;
            if n == 0 {
                return Err(ClientError::Connection("connection closed by peer".to_string()));
            }
            let lines = self
                .splitter
                .push(&self.buf[..n])
                .map_err(|e| ClientError::Connection(format!("protocol violation: {e}")))?;
            if !lines.is_empty() {
                return Ok(lines);
            }
        }
    }

    /// 保留中の要求を、接続の失敗で終わらせる。
    pub(super) fn fail_all(&mut self, reason: &ClientError) {
        for (_, pending) in self.pending.drain() {
            match pending {
                Pending::User(reply) => {
                    let _ = reply.send(Err(reason.clone()));
                }
                Pending::Subscribe { reply, .. } => {
                    let _ = reply.send(Err(reason.clone()));
                }
                Pending::Resub { .. } | Pending::Info => {}
            }
        }
    }
}
