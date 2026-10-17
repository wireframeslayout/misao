//! 接続を持つタスク。要求と応答の対応、購読の登録・再購読、切断後の再接続を 1 つのタスクで順番に処理する
//! （購読の応答は、同じチャンクで続く再生通知より必ず先に反映される）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::conn::{Conn, Pending};
use super::cursor::{StreamCursor, StreamId, StreamPos};
use super::{ClientError, ClientEvent, Cmd, GapReason, Shared, subscribe_params};

pub(super) enum End {
    Closed,
    Lost(ClientError),
}

pub(super) struct Actor {
    pub(super) path: PathBuf,
    pub(super) shared: Arc<Shared>,
    pub(super) cmd_rx: UnboundedReceiver<Cmd>,
    pub(super) cursor: StreamCursor,
    pub(super) info: Option<Value>,
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

pub(super) fn violation(message: impl std::fmt::Display) -> ClientError {
    ClientError::Connection(format!("protocol violation: {message}"))
}

pub(super) fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

impl Actor {
    pub(super) fn emit(&self, event: &ClientEvent) {
        self.shared.emit(event);
    }

    pub(super) async fn run(mut self, mut conn: Conn) {
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

    pub(super) async fn serve(&mut self, conn: &mut Conn) -> End {
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

    pub(super) async fn apply(&mut self, conn: &mut Conn, cmd: Cmd) -> Result<(), ClientError> {
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
    pub(super) fn handle_line(&mut self, conn: &mut Conn, line: &str) -> Result<(), ClientError> {
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

    pub(super) fn settle(
        &mut self,
        pending: Pending,
        outcome: Result<Value, ClientError>,
    ) -> Result<(), ClientError> {
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
    pub(super) fn activate(
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

    pub(super) fn handle_notification(&mut self, method: String, params: Value) -> Result<(), ClientError> {
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
}

pub(super) fn method_of(stream: &StreamId) -> &'static str {
    match stream {
        StreamId::Events => "events.subscribe",
        StreamId::Lines(_) => "pane.subscribe_lines",
    }
}

pub(super) fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or("")
}
