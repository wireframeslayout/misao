//! `--data` の置き場所の検証: 本人の所有・シンボリックリンクでない・group/other が書けない。0755 は許す。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use misao_daemon::{Daemon, DaemonError, DaemonOptions, Limits, LogLevel, Logger};
use tempfile::TempDir;
use tokio::sync::oneshot;

fn options(socket_dir: &Path, data_dir: &Path) -> DaemonOptions {
    DaemonOptions {
        version: None,
        socket_path: socket_dir.join("misao.sock"),
        pid_path: data_dir.join("daemon.pid"),
        state_path: data_dir.join("persistence.json"),
        limits: Limits::default(),
        log: Logger::new(LogLevel::Error, |_| {}),
        profiles: Vec::new(),
    }
}

fn mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[tokio::test]
async fn a_data_directory_created_with_0755_is_accepted_and_files_in_it_are_private() {
    let tmp = TempDir::new().unwrap();
    let socket_dir = tmp.path().join("run");
    let data_dir = tmp.path().join("data");
    fs::create_dir(&data_dir).unwrap();
    mode(&data_dir, 0o755);
    let daemon = Daemon::start(options(&socket_dir, &data_dir)).await.unwrap();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let run = tokio::spawn(daemon.run(async {
        let _ = stop_rx.await;
    }));
    let pid_mode = fs::metadata(data_dir.join("daemon.pid"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        pid_mode & 0o077,
        0,
        "the pid file is not readable by others: {pid_mode:o}"
    );
    stop_tx.send(()).unwrap();
    run.await.unwrap();
    assert_eq!(
        fs::metadata(&data_dir).unwrap().permissions().mode() & 0o777,
        0o755,
        "the mode is left alone"
    );
}

#[tokio::test]
async fn a_data_directory_writable_by_group_or_other_a_symlink_or_a_file_is_refused() {
    let tmp = TempDir::new().unwrap();
    let socket_dir = tmp.path().join("run");
    let data_dir = tmp.path().join("data");
    fs::create_dir(&data_dir).unwrap();
    mode(&data_dir, 0o775);
    let err = Daemon::start(options(&socket_dir, &data_dir))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, DaemonError::DataDirWritableByOthers { .. }),
        "{err}"
    );
    assert!(err.to_string().contains("chmod go-w"), "{err}");
    mode(&data_dir, 0o757);
    assert!(matches!(
        Daemon::start(options(&socket_dir, &data_dir))
            .await
            .err()
            .unwrap(),
        DaemonError::DataDirWritableByOthers { .. }
    ));

    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(
        Daemon::start(options(&socket_dir, &link)).await.err().unwrap(),
        DaemonError::DataDirIsSymlink(_)
    ));
    let file = tmp.path().join("file");
    fs::write(&file, "x").unwrap();
    // 親がファイルだと作れない（ディレクトリでない）
    assert!(
        Daemon::start(options(&socket_dir, &file.join("sub")))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_socket_directory_still_has_to_be_0700() {
    let tmp = TempDir::new().unwrap();
    let socket_dir = tmp.path().join("run");
    fs::create_dir(&socket_dir).unwrap();
    mode(&socket_dir, 0o755);
    let err = Daemon::start(options(&socket_dir, &tmp.path().join("data")))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, DaemonError::SocketDirTooOpen { .. }), "{err}");
}
