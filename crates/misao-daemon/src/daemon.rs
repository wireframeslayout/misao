use std::fs::{self, File};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use misao_protocol::{
    ErrorCode, Incoming, LineSplitter, METHODS, PROTOCOL_SCHEMA, PROTOCOL_VERSION, Request, Response,
    ServerInfo, encode_message, parse_incoming,
};
use nix::fcntl::Flock;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;

use crate::socket::{ensure_socket_dir, listen_unix_socket, lock_socket_start, remove_stale_socket};
use crate::{DaemonError, Logger, acquire_pid_file};

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);

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
    /// 起動の排他ロック。ソケットの掃除と bind からソケットの削除まで、ライフサイクル全体で保持する。
    /// 解放が早いと、終了中の旧デーモンが、後から起動した新デーモン（`--data` が違うと pid ファイルでは排他できない）の
    /// ソケットを unlink できてしまう。
    start_lock: Flock<File>,
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
        // 古いソケットの判定から終了時のソケット削除まで、ソケットの隣の lock で他の起動を締め出す。
        // (pid ファイルは --data 次第で場所が変わるので、ソケットに対する排他はこちらで取る。)
        let (start_lock, listener) = match Self::listen(&socket_path, &log).await {
            Ok(bound) => bound,
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
            start_lock,
            listener,
            socket_path,
            pid_path,
            shared,
        })
    }

    async fn listen(
        socket_path: &std::path::Path,
        log: &Logger,
    ) -> Result<(Flock<File>, UnixListener), DaemonError> {
        let lock = lock_socket_start(socket_path)?;
        if remove_stale_socket(socket_path).await? {
            log.info("removed stale socket");
        }
        // 失敗しても lock はここで解放される（socket は作られていないか、自分で片付けてある）。
        let listener = listen_unix_socket(socket_path)?;
        Ok((lock, listener))
    }

    /// `shutdown` が完了するまで接続を受け付け、完了したら接続を切ってソケットと pid ファイルを消す。
    pub async fn run(self, shutdown: impl Future<Output = ()>) {
        let mut conns = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        conns.spawn(serve_connection(stream, Arc::clone(&self.shared)));
                    }
                    // accept 失敗（EMFILE など）でデーモンごと落ちないよう、その接続だけを諦める。
                    Err(e) => {
                        self.shared.log.error(&format!("server error: {e}"));
                        // EMFILE などが続いても CPU を使い切らない。
                        tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                    }
                },
                Some(_) = conns.join_next() => {}
            }
        }
        self.shared.log.info("shutting down");
        drop(self.listener);
        // 応答を読まないクライアントで send が止まっていても終われるよう、接続のタスクは打ち切る
        // (Node 版の socket.destroy() 相当)。
        conns.abort_all();
        while conns.join_next().await.is_some() {}
        self.shared.log.debug("removing the socket and pid file");
        for f in [&self.socket_path, &self.pid_path] {
            let _ = fs::remove_file(f);
        }
        // ソケットを消した後に解放する。これより前に次のデーモンは起動できない。
        drop(self.start_lock);
    }
}

async fn serve_connection(mut stream: UnixStream, shared: Arc<Shared>) {
    let mut splitter = LineSplitter::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
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
