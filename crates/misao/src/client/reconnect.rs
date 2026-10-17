//! 接続の確立と、切断後の再接続(指数バックオフ)・登録済みストリームの再購読。

use std::time::Duration;

use misao_protocol::PROTOCOL_VERSION;

use serde_json::{Value, json};
use tokio::time::{sleep, timeout};

use super::actor::Actor;
use super::conn::{Conn, Pending};
use super::cursor::{StreamId, StreamPos};
use super::transport::connect_stream;
use super::{ClientError, ClientEvent, Cmd, GapReason, subscribe_params};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(5000);
const INITIAL_DELAY_MS: u64 = 100;
const MAX_DELAY_MS: u64 = 5000;

use super::actor::{major, method_of, str_field, violation};

impl Actor {
    /// 接続して `server.info` を確かめ、登録済みのストリームを再購読する。全体で `CONNECT_TIMEOUT` まで。
    pub(super) async fn establish(&mut self) -> Result<Conn, ClientError> {
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
    pub(super) async fn reconnect(&mut self, first_cause: ClientError) -> Option<Conn> {
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
