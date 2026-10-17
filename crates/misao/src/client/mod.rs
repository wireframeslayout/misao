//! デーモンへの非同期クライアント（Node 版の SDK `MisaoClient` の、CLI が使う範囲）。
//! 切断したら指数バックオフで再接続し、登録済みのストリームを最後の seq（since）から再購読する。
//! デーモンの epoch が変わったら since を捨てて gap を通知する。

mod actor;
mod cursor;
mod transport;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

pub use cursor::{StreamId, StreamPos};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// デーモンが JSON-RPC エラー応答を返した。
    #[error("{message}")]
    Rpc { code: i64, message: String },
    /// 未接続・切断・プロトコル違反など、接続そのものの失敗。
    #[error("{0}")]
    Connection(String),
    /// デーモンとクライアントのプロトコル major が合わない。
    #[error("incompatible protocol version: server {server}, client {client}")]
    IncompatibleVersion { server: String, client: String },
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// デーモン再起動で seq が巻き戻った。
    Epoch,
    /// since が保持範囲より古い（または先）。
    Truncated,
}

/// 接続と購読の出来事。
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    Connected,
    Reconnecting {
        attempt: u32,
        cause: String,
    },
    /// `cause` は再接続先がプロトコル非互換で打ち切ったとき。close() による終了では None。
    Closed {
        cause: Option<String>,
    },
    Gap {
        stream: StreamId,
        reason: GapReason,
    },
    SubscriptionError {
        stream: StreamId,
        message: String,
    },
    /// events / lines の購読の通知（params そのまま）。
    Stream {
        stream: StreamId,
        params: Value,
    },
    /// 購読の対象外の通知（pane.output など）。
    Notification {
        method: String,
        params: Value,
    },
}

pub(crate) enum Cmd {
    Request {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, ClientError>>,
    },
    Subscribe {
        stream: StreamId,
        since: Option<StreamPos>,
        reply: oneshot::Sender<Result<(), ClientError>>,
    },
    Unsubscribe(StreamId),
    Close,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub listeners: Mutex<Vec<UnboundedSender<ClientEvent>>>,
    pub epoch: Mutex<Option<String>>,
}

impl Shared {
    pub fn emit(&self, event: &ClientEvent) {
        let mut listeners = self.listeners.lock().unwrap_or_else(PoisonError::into_inner);
        listeners.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

#[derive(Clone)]
pub struct Client {
    cmd: UnboundedSender<Cmd>,
    shared: Arc<Shared>,
}

impl Client {
    /// 接続して `server.info` で互換を確かめる。
    pub async fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = unbounded_channel();
        actor::start(socket_path, Arc::clone(&shared), rx).await?;
        Ok(Self { cmd: tx, shared })
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let (reply, answer) = oneshot::channel();
        self.send(Cmd::Request {
            method: method.to_string(),
            params,
            reply,
        })?;
        answer.await.map_err(|_| closed_error())?
    }

    /// 要求をその場で送信キューに入れ、応答の受け口を返す。呼んだ順に送られる（`pane.write` の順序を保つため）。
    pub fn request_nowait(
        &self,
        method: &str,
        params: Value,
    ) -> Result<oneshot::Receiver<Result<Value, ClientError>>, ClientError> {
        let (reply, answer) = oneshot::channel();
        self.send(Cmd::Request {
            method: method.to_string(),
            params,
            reply,
        })?;
        Ok(answer)
    }

    /// 現在の epoch（`server.info`）。
    pub fn epoch(&self) -> Option<String> {
        self.shared
            .epoch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 接続と購読の出来事を受ける口。購読した後の出来事から届く。
    pub fn events(&self) -> UnboundedReceiver<ClientEvent> {
        let (tx, rx) = unbounded_channel();
        self.shared
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// デーモンのイベントを購読する。`since` が無ければ購読時点からのライブのみ。
    pub async fn subscribe_events(&self, since: Option<StreamPos>) -> Result<(), ClientError> {
        self.subscribe(StreamId::Events, since).await
    }

    pub async fn subscribe_lines(&self, pane_id: &str, since: Option<StreamPos>) -> Result<(), ClientError> {
        self.subscribe(StreamId::Lines(pane_id.to_string()), since).await
    }

    async fn subscribe(&self, stream: StreamId, since: Option<StreamPos>) -> Result<(), ClientError> {
        let (reply, answer) = oneshot::channel();
        self.send(Cmd::Subscribe { stream, since, reply })?;
        answer.await.map_err(|_| closed_error())?
    }

    pub fn unsubscribe(&self, stream: StreamId) {
        let _ = self.cmd.send(Cmd::Unsubscribe(stream));
    }

    pub fn close(&self) {
        let _ = self.cmd.send(Cmd::Close);
    }

    fn send(&self, cmd: Cmd) -> Result<(), ClientError> {
        self.cmd.send(cmd).map_err(|_| closed_error())
    }
}

fn closed_error() -> ClientError {
    ClientError::Connection("not connected".to_string())
}

/// `{"since":..,"epoch":..}` を購読の params にする。
pub(crate) fn subscribe_params(since: Option<&StreamPos>, pane_id: Option<&str>) -> Value {
    let mut params = json!({});
    if let Some(id) = pane_id {
        params["paneId"] = json!(id);
    }
    if let Some(pos) = since {
        params["since"] = json!(pos.seq);
        params["epoch"] = json!(pos.epoch);
    }
    params
}
