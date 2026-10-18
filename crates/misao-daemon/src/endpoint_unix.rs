//! 接続口（Unix ソケット）。ソケットの隣の lock で他の起動を締め出し、親ディレクトリ 700 とソケット 600 で守る。

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use nix::fcntl::Flock;
use tokio::net::{UnixListener, UnixStream};

use crate::socket::{
    ensure_data_dir, ensure_socket_dir, listen_unix_socket, lock_socket_start, remove_stale_socket,
};
use crate::{DaemonError, Logger};

pub type Connection = UnixStream;

/// 起動の前に、ソケットのディレクトリ（700）と保存した状態の置き場所（`--data`）を用意して検証する。
/// 保存した状態の置き場所は、本人の所有・シンボリックリンクでない・group/other が書けない、を検証する。
/// 0755 の既存のディレクトリは許す（ソケットのディレクトリの 0700 の要件とは別）。
pub fn prepare(socket_path: &Path, state_path: &Path) -> Result<(), DaemonError> {
    ensure_socket_dir(socket_path)?;
    ensure_data_dir(state_path)
}

pub struct Endpoint {
    /// 起動の排他ロック。ソケットの掃除と bind からソケットの削除まで、ライフサイクル全体で保持する。
    /// 解放が早いと、終了中の旧デーモンが、後から起動した新デーモン（`--data` が違うと pid ファイルでは排他できない）の
    /// ソケットを unlink できてしまう。
    _start_lock: Flock<File>,
    listener: Option<UnixListener>,
    path: PathBuf,
}

impl Endpoint {
    /// 古いソケットの判定から終了時のソケット削除まで、ソケットの隣の lock で他の起動を締め出す。
    /// (pid ファイルは --data 次第で場所が変わるので、ソケットに対する排他はこちらで取る。)
    pub async fn bind(path: &Path, log: &Logger) -> Result<Self, DaemonError> {
        let lock = match lock_socket_start(path) {
            // lock は稼働中のデーモンも保持し続ける。待ち受けているなら「起動中」ではなく「稼働中」と伝える。
            Err(DaemonError::StartInProgress(p)) if UnixStream::connect(&p).await.is_ok() => {
                return Err(DaemonError::AlreadyListening(p));
            }
            other => other?,
        };
        if remove_stale_socket(path).await? {
            log.info("removed stale socket");
        }
        // 失敗しても lock はここで解放される（socket は作られていないか、自分で片付けてある）。
        let listener = listen_unix_socket(path)?;
        Ok(Self {
            _start_lock: lock,
            listener: Some(listener),
            path: path.to_path_buf(),
        })
    }

    pub async fn accept(&mut self) -> io::Result<Connection> {
        match &self.listener {
            Some(listener) => listener.accept().await.map(|(stream, _)| stream),
            None => std::future::pending().await,
        }
    }

    /// 新しい接続を受け付けるのをやめる（既存の接続は残る）。
    pub fn stop_listening(&mut self) {
        self.listener = None;
    }

    /// 待ち受けの名残（ソケットファイル）を消す。ロックは `Endpoint` を捨てるまで保持する。
    pub fn remove(&self) {
        let _ = fs::remove_file(&self.path);
    }
}
