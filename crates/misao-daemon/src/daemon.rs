use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::connection::serve_connection;
use crate::core::{Core, CoreOptions};
use crate::endpoint::{self, Endpoint};
use crate::profile::Profile;
use crate::{DaemonError, Limits, Logger, acquire_pid_file};

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);

pub struct DaemonOptions {
    /// デーモンのビルド版（リリースの version）。指定すると `server.info` の version で報告する。
    pub version: Option<String>,
    pub socket_path: PathBuf,
    pub pid_path: PathBuf,
    /// 状態の保存先（`persistence.json`）。
    pub state_path: PathBuf,
    pub limits: Limits,
    pub log: Logger,
    /// pane の稼働判定に使うプロファイル。
    pub profiles: Vec<Arc<Profile>>,
}

pub struct Daemon {
    /// 接続口（Unix ソケット / 名前付きパイプ）。Unix は、ソケットの削除まで起動の排他ロックも持つ。
    endpoint: Endpoint,
    pid_path: PathBuf,
    core: Arc<Core>,
    log: Logger,
}

impl Daemon {
    /// 接続口を用意して listen を始める。失敗時は自分が作ったファイルを片付ける。
    pub async fn start(opts: DaemonOptions) -> Result<Self, DaemonError> {
        let DaemonOptions {
            version,
            socket_path,
            pid_path,
            state_path,
            limits,
            log,
            profiles,
        } = opts;
        endpoint::prepare(&socket_path, &state_path)?;
        acquire_pid_file(&pid_path, std::process::id())?;
        // pid ファイルで排他を取った後に、保存した状態を読む。壊れていれば起動を止める（pid ファイルは残さない）。
        let core = match Core::new(CoreOptions {
            version,
            socket_path: socket_path.clone(),
            limits,
            log: log.clone(),
            state_path: Some(state_path),
            profiles,
        }) {
            Ok(core) => core,
            Err(message) => {
                let _ = fs::remove_file(&pid_path);
                return Err(DaemonError::State(message));
            }
        };
        let endpoint = match Endpoint::bind(&socket_path, &log).await {
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
        Ok(Self {
            endpoint,
            pid_path,
            core,
            log,
        })
    }

    /// `shutdown` が完了するまで接続を受け付け、完了したら接続を切ってソケットと pid ファイルを消す。
    pub async fn run(mut self, shutdown: impl Future<Output = ()>) {
        let mut conns = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                accepted = self.endpoint.accept() => match accepted {
                    Ok(stream) => {
                        conns.spawn(serve_connection(stream, Arc::clone(&self.core)));
                    }
                    // accept 失敗（EMFILE など）でデーモンごと落ちないよう、その接続だけを諦める。
                    Err(e) => {
                        self.log.error(&format!("server error: {e}"));
                        // EMFILE などが続いても CPU を使い切らない。
                        tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                    }
                },
                Some(_) = conns.join_next() => {}
            }
        }
        self.log.info("shutting down");
        self.endpoint.stop_listening();
        // 応答を読まないクライアントで send が止まっていても終われるよう、接続のタスクは打ち切る
        // (Node 版の socket.destroy() 相当)。
        conns.abort_all();
        while conns.join_next().await.is_some() {}
        // 子プロセスを残さない（pid ファイルとソケットが残っている間に、生きている pane を閉じ切る）。
        self.core.shutdown().await;
        self.log.debug("removing the socket and pid file");
        self.endpoint.remove();
        let _ = fs::remove_file(&self.pid_path);
        // 接続口を消した後に解放する（Unix は起動の排他ロック）。これより前に次のデーモンは起動できない。
        drop(self.endpoint);
    }
}
