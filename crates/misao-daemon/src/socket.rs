use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::unistd::getuid;
use tokio::net::{UnixListener, UnixStream};

use crate::DaemonError;

const DIR_MODE: u32 = 0o700;
const SOCKET_MODE: u32 = 0o600;
const GROUP_OTHER_BITS: u32 = 0o077;

fn parent_dir(socket_path: &Path) -> &Path {
    socket_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// ソケットの親ディレクトリを mode 700 で用意する。
/// 既存の場合は、ディレクトリであること・所有者が現在の uid であること・group/other に権限が無いことを
/// 検証し、違反はエラーにする（修正はしない）。
pub fn ensure_socket_dir(socket_path: &Path) -> Result<(), DaemonError> {
    let dir = parent_dir(socket_path);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
        .map_err(|e| DaemonError::io(format!("cannot create socket directory {}", dir.display()), e))?;
    let meta = fs::symlink_metadata(dir)
        .map_err(|e| DaemonError::io(format!("cannot stat socket directory {}", dir.display()), e))?;
    if meta.file_type().is_symlink() {
        return Err(DaemonError::SocketDirIsSymlink(dir.to_path_buf()));
    }
    if !meta.is_dir() {
        return Err(DaemonError::SocketDirNotDirectory(dir.to_path_buf()));
    }
    if meta.uid() != getuid().as_raw() {
        return Err(DaemonError::SocketDirNotOwned(dir.to_path_buf()));
    }
    if meta.mode() & GROUP_OTHER_BITS != 0 {
        return Err(DaemonError::SocketDirTooOpen {
            dir: dir.to_path_buf(),
            mode: meta.mode() & 0o777,
        });
    }
    Ok(())
}

/// 起動の排他ロックの置き場所（ソケットの隣）。
fn lock_path(socket_path: &Path) -> PathBuf {
    let mut name = socket_path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// ソケットの隣の lock ファイルを flock して返す。古いソケットの判定から bind が終わるまで、
/// 返り値（Drop で解除）を保持する。`--data` に依存しないので、pid ファイルの場所が違う起動同士も排他できる。
/// lock ファイルは消さない（消すと別の起動が別 inode をロックして排他が崩れる）。
pub fn lock_socket_start(socket_path: &Path) -> Result<Flock<File>, DaemonError> {
    let path = lock_path(socket_path);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(SOCKET_MODE)
        .open(&path)
        .map_err(|e| DaemonError::io(format!("cannot open {}", path.display()), e))?;
    Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, errno)| match errno {
        Errno::EWOULDBLOCK => DaemonError::StartInProgress(socket_path.to_path_buf()),
        other => DaemonError::io(format!("cannot lock {}", path.display()), io::Error::from(other)),
    })
}

/// Unix ソケットで listen し、ソケットファイルを mode 600 にする。
///
/// 接続の可否は親ディレクトリ（`ensure_socket_dir` で 700 を検証済み）で守り、listen 後に chmod 600 する。
/// umask はプロセス全体に効くため操作しない。chmod に失敗したらソケットを閉じてから失敗する。
pub fn listen_unix_socket(socket_path: &Path) -> Result<UnixListener, DaemonError> {
    let listener = UnixListener::bind(socket_path)
        .map_err(|e| DaemonError::io(format!("cannot listen on {}", socket_path.display()), e))?;
    // ここに来た時点でソケットファイルは自分が作ったもの。chmod に失敗したら片付ける。
    // bind 自体の失敗ではパスを消さない（先に起動した稼働中のデーモンのソケットかもしれない）。
    if let Err(e) = fs::set_permissions(socket_path, fs::Permissions::from_mode(SOCKET_MODE)) {
        drop(listener);
        let _ = fs::remove_file(socket_path);
        return Err(DaemonError::io(
            format!("cannot chmod {}", socket_path.display()),
            e,
        ));
    }
    Ok(listener)
}

fn errno_name(e: &io::Error) -> String {
    e.raw_os_error()
        .map_or_else(|| "UNKNOWN".to_string(), |n| format!("{:?}", Errno::from_raw(n)))
}

/// 応答するプロセスがいない古いソケットファイルを消す。消したら true。
/// 接続できたら（稼働中のデーモンがいる）、または古いと断定できないエラー（EACCES など）なら失敗する。
/// 消してよいのは ECONNREFUSED（待ち受けなし）だけ。ENOENT はそもそも無いので何もしない。
pub async fn remove_stale_socket(socket_path: &Path) -> Result<bool, DaemonError> {
    match UnixStream::connect(socket_path).await {
        Ok(_) => Err(DaemonError::AlreadyListening(socket_path.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            fs::remove_file(socket_path)
                .map_err(|e| DaemonError::io(format!("cannot remove {}", socket_path.display()), e))?;
            Ok(true)
        }
        Err(e) => Err(DaemonError::SocketStateUnknown {
            path: socket_path.to_path_buf(),
            code: errno_name(&e),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().mode() & 0o777
    }

    #[test]
    fn creates_directory_with_mode_700() {
        let root = TempDir::new().unwrap();
        let sock = root.path().join("a/b/misao.sock");
        ensure_socket_dir(&sock).unwrap();
        assert_eq!(mode(&root.path().join("a/b")), 0o700);
    }

    #[test]
    fn rejects_group_other_access_without_fixing_it() {
        let root = TempDir::new().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o750)).unwrap();
        let err = ensure_socket_dir(&root.path().join("misao.sock")).unwrap_err();
        assert!(
            err.to_string().contains("accessible by group/other (mode 750)"),
            "{err}"
        );
        assert_eq!(mode(root.path()), 0o750);
    }

    #[test]
    fn rejects_symlinked_directory() {
        let root = TempDir::new().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.path().join("link");
        symlink(&real, &link).unwrap();
        let err = ensure_socket_dir(&link.join("misao.sock")).unwrap_err();
        assert!(matches!(err, DaemonError::SocketDirIsSymlink(_)), "{err}");
    }

    #[test]
    fn second_start_lock_is_refused_until_the_first_is_dropped() {
        let root = TempDir::new().unwrap();
        let sock = root.path().join("misao.sock");
        let first = lock_socket_start(&sock).unwrap();
        let err = lock_socket_start(&sock).unwrap_err();
        assert!(matches!(err, DaemonError::StartInProgress(_)), "{err}");
        drop(first);
        assert!(lock_socket_start(&sock).is_ok());
    }

    #[tokio::test]
    async fn bind_failure_does_not_remove_the_path() {
        let root = TempDir::new().unwrap();
        let sock = root.path().join("misao.sock");
        let _live = listen_unix_socket(&sock).unwrap();
        assert!(listen_unix_socket(&sock).is_err());
        assert!(
            sock.exists(),
            "the live daemon's socket must survive a failed bind"
        );
    }

    #[tokio::test]
    async fn listen_sets_socket_mode_600() {
        let root = TempDir::new().unwrap();
        let sock = root.path().join("misao.sock");
        let _listener = listen_unix_socket(&sock).unwrap();
        assert_eq!(mode(&sock), 0o600);
    }

    #[tokio::test]
    async fn stale_socket_is_removed_when_nobody_listens() {
        let root = TempDir::new().unwrap();
        let sock = root.path().join("misao.sock");
        drop(listen_unix_socket(&sock).unwrap()); // ファイルは残る
        assert!(sock.exists());
        assert!(remove_stale_socket(&sock).await.unwrap());
        assert!(!sock.exists());
    }

    #[tokio::test]
    async fn missing_socket_is_a_no_op() {
        let root = TempDir::new().unwrap();
        assert!(!remove_stale_socket(&root.path().join("none.sock")).await.unwrap());
    }

    #[tokio::test]
    async fn live_socket_is_refused() {
        let root = TempDir::new().unwrap();
        let sock = root.path().join("misao.sock");
        let _listener = listen_unix_socket(&sock).unwrap();
        let err = remove_stale_socket(&sock).await.unwrap_err();
        assert!(
            err.to_string()
                .starts_with("another daemon is already listening on"),
            "{err}"
        );
        assert!(sock.exists());
    }
}
