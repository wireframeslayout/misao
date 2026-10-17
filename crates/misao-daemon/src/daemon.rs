use std::fs::{self, File};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nix::fcntl::Flock;
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;

use crate::connection::serve_connection;
use crate::core::{Core, CoreOptions};
use crate::socket::{ensure_socket_dir, listen_unix_socket, lock_socket_start, remove_stale_socket};
use crate::{DaemonError, Limits, Logger, acquire_pid_file};

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);

pub struct DaemonOptions {
    /// デーモンのビルド版（リリースの version）。指定すると `server.info` の version で報告する。
    pub version: Option<String>,
    pub socket_path: PathBuf,
    pub pid_path: PathBuf,
    pub limits: Limits,
    pub log: Logger,
}

pub struct Daemon {
    /// 起動の排他ロック。ソケットの掃除と bind からソケットの削除まで、ライフサイクル全体で保持する。
    /// 解放が早いと、終了中の旧デーモンが、後から起動した新デーモン（`--data` が違うと pid ファイルでは排他できない）の
    /// ソケットを unlink できてしまう。
    start_lock: Flock<File>,
    listener: UnixListener,
    socket_path: PathBuf,
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
            limits,
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
        let core = Core::new(CoreOptions {
            version,
            socket_path: socket_path.clone(),
            limits,
            log: log.clone(),
        });
        Ok(Self {
            start_lock,
            listener,
            socket_path,
            pid_path,
            core,
            log,
        })
    }

    async fn listen(
        socket_path: &std::path::Path,
        log: &Logger,
    ) -> Result<(Flock<File>, UnixListener), DaemonError> {
        let lock = match lock_socket_start(socket_path) {
            // lock は稼働中のデーモンも保持し続ける。待ち受けているなら「起動中」ではなく「稼働中」と伝える。
            Err(DaemonError::StartInProgress(path)) if UnixStream::connect(&path).await.is_ok() => {
                return Err(DaemonError::AlreadyListening(path));
            }
            other => other?,
        };
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
        drop(self.listener);
        // 応答を読まないクライアントで send が止まっていても終われるよう、接続のタスクは打ち切る
        // (Node 版の socket.destroy() 相当)。
        conns.abort_all();
        while conns.join_next().await.is_some() {}
        self.log.debug("removing the socket and pid file");
        for f in [&self.socket_path, &self.pid_path] {
            let _ = fs::remove_file(f);
        }
        // ソケットを消した後に解放する。これより前に次のデーモンは起動できない。
        drop(self.start_lock);
    }
}
