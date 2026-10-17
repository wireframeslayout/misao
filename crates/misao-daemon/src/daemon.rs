use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use misao_protocol::{
    ErrorCode, Incoming, LineSplitter, METHODS, PROTOCOL_SCHEMA, PROTOCOL_VERSION, Request, Response,
    ServerInfo, encode_message, parse_incoming,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::socket::{ensure_socket_dir, listen_unix_socket, remove_stale_socket};
use crate::{DaemonError, Logger, acquire_pid_file};

pub struct DaemonOptions {
    /// デーモンのビルド版（リリースの version）。指定すると `server.info` の version で報告する。
    pub version: Option<String>,
    pub socket_path: PathBuf,
    pub pid_path: PathBuf,
    pub log: Logger,
}

/// 接続間で共有する、起動時に決まる値。
struct Shared {
    version: Option<String>,
    /// 起動ごとに変わる ID。seq の巻き戻り（デーモン再起動）をクライアントが検知するためのもの。
    epoch: String,
    started_at: Instant,
    /// イベントリングの先頭 seq。起動時の `daemon.started` が 1 件目。
    event_head: u64,
    log: Logger,
}

pub struct Daemon {
    listener: UnixListener,
    socket_path: PathBuf,
    pid_path: PathBuf,
    shared: Arc<Shared>,
}

impl Daemon {
    /// 接続口を用意して listen を始める。失敗時は自分が作ったファイルを片付ける。
    pub async fn start(opts: DaemonOptions) -> Result<Self, DaemonError> {
        let DaemonOptions {
            version,
            socket_path,
            pid_path,
            log,
        } = opts;
        ensure_socket_dir(&socket_path)?;
        acquire_pid_file(&pid_path, std::process::id())?;
        // pid ファイルで排他を取った後は、socket_path にあるファイルを自分のものとして扱える。
        let listener = match Self::listen(&socket_path, &log).await {
            Ok(listener) => listener,
            Err(e) => {
                let _ = fs::remove_file(&pid_path);
                return Err(e);
            }
        };
        log.info(&format!(
            "listening on {} (pid {})",
            socket_path.display(),
            std::process::id()
        ));
        let shared = Arc::new(Shared {
            version,
            epoch: ulid::Ulid::new().to_string(),
            started_at: Instant::now(),
            event_head: 1,
            log,
        });
        Ok(Self {
            listener,
            socket_path,
            pid_path,
            shared,
        })
    }

    async fn listen(socket_path: &std::path::Path, log: &Logger) -> Result<UnixListener, DaemonError> {
        if remove_stale_socket(socket_path).await? {
            log.info("removed stale socket");
        }
        listen_unix_socket(socket_path).inspect_err(|_| {
            let _ = fs::remove_file(socket_path);
        })
    }

    /// `shutdown` が完了するまで接続を受け付け、完了したら接続を切ってソケットと pid ファイルを消す。
    pub async fn run(self, shutdown: impl Future<Output = ()>) {
        let (stop_tx, stop_rx) = watch::channel(false);
        let mut conns = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        conns.spawn(serve_connection(stream, Arc::clone(&self.shared), stop_rx.clone()));
                    }
                    // accept 失敗（EMFILE など）でデーモンごと落ちないよう、その接続だけを諦める。
                    Err(e) => self.shared.log.error(&format!("server error: {e}")),
                },
                Some(_) = conns.join_next() => {}
            }
        }
        self.shared.log.info("shutting down");
        let _ = stop_tx.send(true);
        drop(self.listener);
        while conns.join_next().await.is_some() {}
        for f in [&self.socket_path, &self.pid_path] {
            let _ = fs::remove_file(f);
        }
    }
}

async fn serve_connection(mut stream: UnixStream, shared: Arc<Shared>, mut stop: watch::Receiver<bool>) {
    let mut splitter = LineSplitter::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = tokio::select! {
            _ = stop.changed() => return,
            read = stream.read(&mut buf) => match read {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            },
        };
        let lines = match splitter.push(&buf[..n]) {
            Ok(lines) => lines,
            Err(e) => {
                // 行の上限超過など、続行できない入力。Parse error を返して切る。
                let _ = send(&mut stream, &Response::parse_error(e.to_string())).await;
                let _ = stream.shutdown().await;
                return;
            }
        };
        for line in lines {
            if let Some(response) = respond(&shared, &line)
                && send(&mut stream, &response).await.is_err()
            {
                return;
            }
        }
    }
}

async fn send(stream: &mut UnixStream, response: &Response) -> std::io::Result<()> {
    let bytes = encode_message(response).map_err(std::io::Error::other)?;
    stream.write_all(&bytes).await
}

/// 1 行ぶんの入力への応答。クライアントからの通知・応答は無視する（None）。
fn respond(shared: &Shared, line: &str) -> Option<Response> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => return Some(Response::parse_error(e.to_string())),
    };
    match parse_incoming(&value) {
        Incoming::Ignored => None,
        Incoming::Invalid { id } => Some(Response::invalid_request(id)),
        Incoming::Request(req) => {
            let id = req.id.clone();
            Some(match dispatch(shared, &req) {
                Ok(result) => Response::success(id, result),
                Err((code, message)) => {
                    if code == ErrorCode::INTERNAL {
                        shared
                            .log
                            .error(&format!("internal error in {}: {message}", req.method));
                    }
                    Response::error(Some(id), code, message)
                }
            })
        }
    }
}

fn dispatch(shared: &Shared, req: &Request) -> Result<Value, (i64, String)> {
    let method = req.method.as_str();
    if !METHODS.contains(&method) {
        return Err((ErrorCode::METHOD_NOT_FOUND, format!("unknown method: {method}")));
    }
    match method {
        "server.info" => serde_json::to_value(server_info(shared)).map_err(internal),
        "server.schema" => serde_json::from_str(PROTOCOL_SCHEMA).map_err(internal),
        _ => Err((ErrorCode::NOT_IMPLEMENTED, format!("not implemented: {method}"))),
    }
}

fn internal(e: serde_json::Error) -> (i64, String) {
    (ErrorCode::INTERNAL, e.to_string())
}

fn server_info(shared: &Shared) -> ServerInfo {
    ServerInfo {
        protocol_version: PROTOCOL_VERSION.to_string(),
        version: shared.version.clone(),
        pid: std::process::id(),
        epoch: shared.epoch.clone(),
        uptime_sec: shared.started_at.elapsed().as_secs(),
        pane_count: 0,
        event_head: shared.event_head,
    }
}
