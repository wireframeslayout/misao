//! 起動の lock は、デーモンのライフサイクル全体（ソケットの削除まで）で保持される。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::mpsc;

use misao_daemon::{Daemon, DaemonError, DaemonOptions, Limits, LogLevel, Logger};
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::sync::oneshot;

const REMOVING: &str = "removing the socket and pid file";

fn options(dir: &Path, pid_name: &str, log: Logger) -> DaemonOptions {
    DaemonOptions {
        version: None,
        socket_path: dir.join("misao.sock"),
        pid_path: dir.join(pid_name),
        state_path: dir.join("persistence.json"),
        limits: Limits::default(),
        log,
        profiles: Vec::new(),
    }
}

fn quiet() -> Logger {
    Logger::new(LogLevel::Error, |_| {})
}

/// 終了中の旧デーモンが、ソケットを消す直前で止まっている間に、pid ファイルの場所が違う新デーモンを起動する。
/// lock が bind の完了で解放される実装だと、新デーモンは旧デーモンの（もう待ち受けていない）ソケットを
/// 古いものとして消して自分のソケットを作り、その直後に旧デーモンが新デーモンのソケットを unlink してしまう。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_daemon_cannot_start_while_the_old_one_is_still_removing_its_socket() {
    let tmp = TempDir::new().unwrap();
    let run_dir = tmp.path().join("run"); // 700 で作られる（TempDir 自体は 755 でソケットの置き場所にできない）
    let dir = run_dir.as_path();
    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    let resume_rx = std::sync::Mutex::new(resume_rx);
    let log = Logger::new(LogLevel::Debug, move |msg| {
        if msg == REMOVING {
            paused_tx.send(()).unwrap();
            resume_rx.lock().unwrap().recv().unwrap();
        }
    });
    let old = Daemon::start(options(dir, "old.pid", log)).await.unwrap();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let old_run = tokio::spawn(old.run(async {
        let _ = stop_rx.await;
    }));
    stop_tx.send(()).unwrap();
    tokio::task::spawn_blocking(move || paused_rx.recv().unwrap())
        .await
        .unwrap();

    let started = Daemon::start(options(dir, "new.pid", quiet())).await;
    assert!(
        matches!(started, Err(DaemonError::StartInProgress(_))),
        "the start lock must be held until the old daemon has removed its socket"
    );
    assert!(
        !dir.join("new.pid").exists(),
        "a refused start leaves no pid file"
    );

    resume_tx.send(()).unwrap();
    old_run.await.unwrap();
    assert!(!dir.join("misao.sock").exists());

    // 旧デーモンが完全に終わったあとは起動でき、ソケットは新デーモンのもの。
    let new = Daemon::start(options(dir, "new.pid", quiet())).await.unwrap();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let new_run = tokio::spawn(new.run(async {
        let _ = stop_rx.await;
    }));
    assert!(UnixStream::connect(dir.join("misao.sock")).await.is_ok());
    stop_tx.send(()).unwrap();
    new_run.await.unwrap();
}

/// 終了と再起動を重ねても、新しいデーモンのソケットが消されない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_restarts_always_leave_a_connectable_socket() {
    let tmp = TempDir::new().unwrap();
    let run_dir = tmp.path().join("run"); // 700 で作られる（TempDir 自体は 755 でソケットの置き場所にできない）
    let dir = run_dir.as_path();
    for round in 0..20 {
        let daemon = Daemon::start(options(dir, &format!("{round}.pid"), quiet()))
            .await
            .unwrap();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let run = tokio::spawn(daemon.run(async {
            let _ = stop_rx.await;
        }));
        assert!(
            UnixStream::connect(dir.join("misao.sock")).await.is_ok(),
            "round {round}"
        );
        stop_tx.send(()).unwrap();
        run.await.unwrap();
    }
}
