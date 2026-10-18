use std::io;
use std::path::PathBuf;

/// デーモンの起動・終了で起きる失敗。メッセージは Node 版（`packages/daemon`）に合わせる。
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("socket directory {} is a symbolic link; use the real path", .0.display())]
    SocketDirIsSymlink(PathBuf),
    #[error("socket directory {} is not a directory", .0.display())]
    SocketDirNotDirectory(PathBuf),
    #[error("socket directory {} is not owned by the current user", .0.display())]
    SocketDirNotOwned(PathBuf),
    #[error(
        "socket directory {dir} is accessible by group/other (mode {mode:o}); run: chmod 700 {dir}",
        dir = .dir.display()
    )]
    SocketDirTooOpen { dir: PathBuf, mode: u32 },
    #[error("data directory {} is a symbolic link; use the real path", .0.display())]
    DataDirIsSymlink(PathBuf),
    #[error("data directory {} is not a directory", .0.display())]
    DataDirNotDirectory(PathBuf),
    #[error("data directory {} is not owned by the current user", .0.display())]
    DataDirNotOwned(PathBuf),
    #[error(
        "data directory {dir} is writable by group/other (mode {mode:o}); run: chmod go-w {dir}",
        dir = .dir.display()
    )]
    DataDirWritableByOthers { dir: PathBuf, mode: u32 },
    #[error(
        "data directory {dir} grants write access to {principal}; remove it with: icacls \"{dir}\" /remove:g {principal}",
        dir = .dir.display()
    )]
    DataDirWritableByPrincipal { dir: PathBuf, principal: String },
    #[error("another daemon is already listening on {}", .0.display())]
    AlreadyListening(PathBuf),
    #[error("cannot determine whether {} is in use ({code}); refusing to remove it", .path.display())]
    SocketStateUnknown { path: PathBuf, code: String },
    #[error("another daemon is starting on {}; try again once it is up", .0.display())]
    StartInProgress(PathBuf),
    #[error("another daemon (pid {pid}) holds {}", .path.display())]
    PidFileHeld { pid: i32, path: PathBuf },
    #[error("could not acquire {}: another process keeps recreating it", .0.display())]
    PidFileContended(PathBuf),
    /// 保存した状態（`persistence.json`）が読めない。
    #[error("{0}")]
    State(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

impl DaemonError {
    pub(crate) fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
