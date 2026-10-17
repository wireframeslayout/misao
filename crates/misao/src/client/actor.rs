//! 接続を持つタスク。要求と応答の対応、購読の登録・再購読、切断後の再接続を 1 つのタスクで順番に処理する
//! （購読の応答は、同じチャンクで続く再生通知より必ず先に反映される）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use misao_protocol::{LineSplitter, PROTOCOL_VERSION, encode_message};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

use super::cursor::{StreamCursor, StreamId, StreamPos};
use super::transport::{Stream, connect_stream};
use super::{ClientError, ClientEvent, Cmd, GapReason, Shared, subscribe_params};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(5000);
const INITIAL_DELAY_MS: u64 = 100;
const MAX_DELAY_MS: u64 = 5000;

enum Pending {
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

struct Conn {
    read: ReadHalf<Stream>,
    write: WriteHalf<Stream>,
    splitter: LineSplitter,
    pending: HashMap<u64, Pending>,
    next_id: u64,
    buf: Vec<u8>,
}

impl Conn {
    fn new(stream: Stream) -> Self {
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

    async fn send(&mut self, method: &str, params: Value, pending: Pending) -> Result<(), ClientError> {
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
    async fn read_lines(&mut self) -> Result<Vec<String>, ClientError> {
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
    fn fail_all(&mut self, reason: &ClientError) {
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

enum End {
    Closed,
    Lost(ClientError),
}

struct Actor {
    path: PathBuf,
    shared: Arc<Shared>,
    cmd_rx: UnboundedReceiver<Cmd>,
    cursor: StreamCursor,
    info: Option<Value>,
}

pub(super) async fn start(
    path: &Path,
    shared: Arc<Shared>,
    cmd_rx: UnboundedReceiver<Cmd>,
) -> Result<(), ClientError> {
    let mut actor = Actor {
        path: path.to_path_buf(),
        shared,
        cmd_rx,
        cursor: StreamCursor::default(),
        info: None,
    };
    let conn = actor.establish().await?;
    tokio::spawn(actor.run(conn));
    Ok(())
}

fn violation(message: impl std::fmt::Display) -> ClientError {
    ClientError::Connection(format!("protocol violation: {message}"))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

impl Actor {
    fn emit(&self, event: &ClientEvent) {
        self.shared.emit(event);
    }

    async fn run(mut self, mut conn: Conn) {
        loop {
            match self.serve(&mut conn).await {
                End::Closed => {
                    self.emit(&ClientEvent::Closed { cause: None });
                    return;
                }
                End::Lost(cause) => {
                    conn.fail_all(&cause);
                    drop(conn);
                    match self.reconnect(cause).await {
                        Some(next) => conn = next,
                        None => return,
                    }
                }
            }
        }
    }

    async fn serve(&mut self, conn: &mut Conn) -> End {
        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => match cmd {
                    None | Some(Cmd::Close) => return End::Closed,
                    Some(cmd) => if let Err(e) = self.apply(conn, cmd).await { return End::Lost(e) },
                },
                read = conn.read_lines() => match read {
                    Ok(lines) => for line in lines {
                        if let Err(e) = self.handle_line(conn, &line) { return End::Lost(e) }
                    },
                    Err(e) => return End::Lost(e),
                },
            }
        }
    }

    async fn apply(&mut self, conn: &mut Conn, cmd: Cmd) -> Result<(), ClientError> {
        match cmd {
            Cmd::Request {
                method,
                params,
                reply,
            } => conn.send(&method, params, Pending::User(reply)).await,
            Cmd::Subscribe { stream, since, reply } => {
                let is_pending = conn
                    .pending
                    .values()
                    .any(|p| matches!(p, Pending::Subscribe { stream: s, .. } if *s == stream));
                if self.cursor.contains(&stream) || is_pending {
                    let _ = reply.send(Err(ClientError::Other(format!(
                        "stream already registered: {}",
                        stream.key()
                    ))));
                    return Ok(());
                }
                let params = subscribe_params(since.as_ref(), stream.pane_id());
                let method = method_of(&stream);
                conn.send(method, params, Pending::Subscribe { stream, since, reply })
                    .await
            }
            Cmd::Unsubscribe(stream) => {
                self.cursor.remove(&stream);
                Ok(())
            }
            Cmd::Close => Ok(()),
        }
    }

    /// 1 行を処理する。Err はプロトコル違反（接続を切って再接続する）。
    fn handle_line(&mut self, conn: &mut Conn, line: &str) -> Result<(), ClientError> {
        let value: Value = serde_json::from_str(line).map_err(|_| violation("invalid JSON line"))?;
        if value.get("method").is_some() && value.get("id").is_none() {
            let method = str_field(&value, "method").unwrap_or_default().to_string();
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            return self.handle_notification(method, params);
        }
        if value.get("method").is_some() {
            return Err(violation("unexpected request from server"));
        }
        let id = value.get("id").and_then(Value::as_u64);
        let pending = id.and_then(|id| conn.pending.remove(&id));
        let Some(pending) = pending else {
            return Err(violation(format!(
                "response for unknown id {}",
                value.get("id").unwrap_or(&Value::Null)
            )));
        };
        let outcome = match value.get("error") {
            Some(err) => Err(ClientError::Rpc {
                code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: str_field(err, "message").unwrap_or_default().to_string(),
            }),
            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
        };
        self.settle(pending, outcome)
    }

    fn settle(&mut self, pending: Pending, outcome: Result<Value, ClientError>) -> Result<(), ClientError> {
        match pending {
            Pending::User(reply) => {
                let _ = reply.send(outcome);
            }
            Pending::Info => self.info = outcome.ok(),
            Pending::Subscribe { stream, since, reply } => match outcome {
                Ok(result) => match self.activate(&stream, since.as_ref(), &result) {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e.clone()));
                        return Err(e);
                    }
                },
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            },
            Pending::Resub { stream, was_reset } => match outcome {
                Ok(result) => {
                    if result.get("gap").and_then(Value::as_bool) == Some(true) && !was_reset {
                        self.emit(&ClientEvent::Gap {
                            stream,
                            reason: GapReason::Truncated,
                        });
                    }
                }
                Err(ClientError::Rpc { message, .. }) => {
                    self.cursor.remove(&stream);
                    self.emit(&ClientEvent::SubscriptionError { stream, message });
                }
                Err(e) => return Err(e),
            },
        }
        Ok(())
    }

    /// 購読の応答を反映する。since が無ければ head から、あれば epoch が現在と違うかを比べる。
    fn activate(
        &mut self,
        stream: &StreamId,
        since: Option<&StreamPos>,
        result: &Value,
    ) -> Result<(), ClientError> {
        let (Some(head), Some(gap), Some(epoch)) = (
            result.get("head").and_then(Value::as_u64),
            result.get("gap").and_then(Value::as_bool),
            str_field(result, "epoch"),
        ) else {
            return Err(violation(format!("invalid result for {}", method_of(stream))));
        };
        let epoch_changed = since.is_some_and(|s| s.epoch != epoch);
        // epoch が違うと最古から再生されるので、古い since は引き継がない。
        // since が head より先なら head に寄せる（先の seq を重複扱いで捨てないため）。
        let last = if epoch_changed {
            0
        } else {
            since.map_or(head, |s| s.seq).min(head)
        };
        self.cursor.add(stream.clone(), last);
        if gap {
            let reason = if epoch_changed {
                GapReason::Epoch
            } else {
                GapReason::Truncated
            };
            self.emit(&ClientEvent::Gap {
                stream: stream.clone(),
                reason,
            });
        }
        Ok(())
    }

    fn handle_notification(&mut self, method: String, params: Value) -> Result<(), ClientError> {
        let stream = match method.as_str() {
            "event" => Some(StreamId::Events),
            "pane.line" => str_field(&params, "paneId").map(|id| StreamId::Lines(id.to_string())),
            _ => None,
        };
        let Some(stream) = stream else {
            self.emit(&ClientEvent::Notification { method, params });
            return Ok(());
        };
        let seq = params
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| violation(format!("invalid {method} params")))?;
        if self.cursor.accept(&stream, seq) {
            self.emit(&ClientEvent::Stream { stream, params });
        }
        Ok(())
    }

    /// 接続して `server.info` を確かめ、登録済みのストリームを再購読する。全体で `CONNECT_TIMEOUT` まで。
    async fn establish(&mut self) -> Result<Conn, ClientError> {
        match timeout(CONNECT_TIMEOUT, self.establish_inner()).await {
            Ok(result) => result,
            Err(_) => Err(ClientError::Connection(format!(
                "connection setup timed out after {}ms",
                CONNECT_TIMEOUT.as_millis()
            ))),
        }
    }

    async fn establish_inner(&mut self) -> Result<Conn, ClientError> {
        let stream = connect_stream(&self.path).await.map_err(|e| {
            ClientError::Connection(format!("cannot connect to {}: {e}", self.path.display()))
        })?;
        let mut conn = Conn::new(stream);
        self.info = None;
        conn.send("server.info", json!({}), Pending::Info).await?;
        while self.info.is_none() {
            for line in conn.read_lines().await? {
                self.handle_line(&mut conn, &line)?;
            }
        }
        let info = self.info.take().unwrap_or(Value::Null);
        let (Some(server), Some(epoch)) = (str_field(&info, "protocolVersion"), str_field(&info, "epoch"))
        else {
            return Err(violation("invalid result for server.info"));
        };
        if major(server) != major(PROTOCOL_VERSION) {
            return Err(ClientError::IncompatibleVersion {
                server: server.to_string(),
                client: PROTOCOL_VERSION.to_string(),
            });
        }
        *self
            .shared
            .epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(epoch.to_string());
        let reset = self.cursor.reset_for_epoch(epoch);
        for stream in &reset {
            self.emit(&ClientEvent::Gap {
                stream: stream.clone(),
                reason: GapReason::Epoch,
            });
        }
        self.resubscribe(&mut conn, epoch, &reset).await?;
        Ok(conn)
    }

    async fn resubscribe(
        &mut self,
        conn: &mut Conn,
        epoch: &str,
        reset: &[StreamId],
    ) -> Result<(), ClientError> {
        for stream in self.cursor.streams() {
            let since = StreamPos {
                seq: self.cursor.last_seq(&stream).unwrap_or(0),
                epoch: epoch.to_string(),
            };
            let params = subscribe_params(Some(&since), stream.pane_id());
            let was_reset = reset.contains(&stream);
            conn.send(method_of(&stream), params, Pending::Resub { stream, was_reset })
                .await?;
        }
        while conn.pending.values().any(|p| matches!(p, Pending::Resub { .. })) {
            for line in conn.read_lines().await? {
                self.handle_line(conn, &line)?;
            }
        }
        Ok(())
    }

    /// 指数バックオフで再接続する。close() されるか、再接続先が非互換なら None。
    async fn reconnect(&mut self, first_cause: ClientError) -> Option<Conn> {
        let mut cause = first_cause;
        for attempt in 1u32.. {
            let delay = INITIAL_DELAY_MS
                .saturating_mul(1u64 << (attempt - 1).min(20))
                .min(MAX_DELAY_MS);
            self.emit(&ClientEvent::Reconnecting {
                attempt,
                cause: cause.to_string(),
            });
            if !self.sleep_serving_commands(Duration::from_millis(delay)).await {
                self.emit(&ClientEvent::Closed { cause: None });
                return None;
            }
            match self.establish().await {
                Ok(conn) => {
                    self.emit(&ClientEvent::Connected);
                    return Some(conn);
                }
                Err(e @ ClientError::IncompatibleVersion { .. }) => {
                    // 非互換は待っても直らない。打ち切って利用側へ知らせる。
                    self.emit(&ClientEvent::Closed {
                        cause: Some(e.to_string()),
                    });
                    return None;
                }
                Err(e) => cause = e,
            }
        }
        None
    }

    /// 待つ間、要求は「未接続」で断る。close() されたら false。
    async fn sleep_serving_commands(&mut self, delay: Duration) -> bool {
        let wake = sleep(delay);
        tokio::pin!(wake);
        loop {
            tokio::select! {
                () = &mut wake => return true,
                cmd = self.cmd_rx.recv() => match cmd {
                    None | Some(Cmd::Close) => return false,
                    Some(Cmd::Request { reply, .. }) => {
                        let _ = reply.send(Err(ClientError::Connection("not connected".to_string())));
                    }
                    Some(Cmd::Subscribe { reply, .. }) => {
                        let _ = reply.send(Err(ClientError::Connection("not connected".to_string())));
                    }
                    Some(Cmd::Unsubscribe(stream)) => self.cursor.remove(&stream),
                },
            }
        }
    }
}

fn method_of(stream: &StreamId) -> &'static str {
    match stream {
        StreamId::Events => "events.subscribe",
        StreamId::Lines(_) => "pane.subscribe_lines",
    }
}

fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or("")
}
