//! Windows の接続口（名前付きパイプ）: 起動したユーザーだけの ACL、リモートの拒否、最初のインスタンスの確保。
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use misao_daemon::{Daemon, DaemonError, DaemonOptions, Limits, LogLevel, Logger};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::oneshot;

fn pipe_name(tag: &str) -> String {
    format!("misao-test-{}-{tag}", std::process::id())
}

fn options(dir: &Path, name: &str) -> DaemonOptions {
    DaemonOptions {
        version: None,
        socket_path: PathBuf::from(format!("\\\\.\\pipe\\{name}")),
        pid_path: dir.join("daemon.pid"),
        state_path: dir.join("persistence.json"),
        limits: Limits::default(),
        log: Logger::new(LogLevel::Error, |_| {}),
        profiles: Vec::new(),
    }
}

/// 起動して、止める合図と終了待ちを返す。
async fn run(options: DaemonOptions) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let daemon = Daemon::start(options).await.unwrap();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(daemon.run(async {
        let _ = stop_rx.await;
    }));
    (stop_tx, handle)
}

fn powershell(script: &str) -> String {
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 現在のユーザーの SID（`whoami /user` の 2 列目）。
fn current_sid() -> String {
    let out = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.trim()
        .split(',')
        .nth(1)
        .unwrap()
        .trim_matches('"')
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pipe_grants_access_to_the_current_user_only() {
    let tmp = TempDir::new().unwrap();
    let name = pipe_name("acl");
    let (stop, handle) = run(options(tmp.path(), &name)).await;

    let sddl = tokio::task::spawn_blocking(move || {
        powershell(&format!(
            "$p = [System.IO.Pipes.NamedPipeClientStream]::new('.', '{name}', [System.IO.Pipes.PipeDirection]::InOut); \
             $p.Connect(5000); $p.GetAccessControl().GetSecurityDescriptorSddlForm('Access'); $p.Dispose()"
        ))
    })
    .await
    .unwrap();

    let sid = current_sid();
    // SDDL は RID 500 の SID を `LA`（組み込みの Administrator）に略す（CI のランナーのユーザーがそれ）。
    let trustee = if sid.ends_with("-500") {
        "LA".to_string()
    } else {
        sid.clone()
    };
    // `D:P(A;;FA;;;<sid>)` だけ。Everyone (WD)、匿名 (AN)、SYSTEM (SY)、管理者 (BA) などは入っていない。
    assert!(
        sddl.starts_with("D:P"),
        "the DACL is protected (no inheritance): {sddl}"
    );
    assert_eq!(sddl.matches("(A;").count(), 1, "exactly one allow entry: {sddl}");
    assert!(
        sddl.contains(&format!(";;;{trustee})")),
        "only {sid} ({trustee}): {sddl}"
    );
    for other in ["WD", "AN", "SY", "BA", "AU", "BU", "IU"] {
        assert!(
            !sddl.contains(&format!(";;;{other})")),
            "{other} must not be granted: {sddl}"
        );
    }
    let _ = stop.send(());
    handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_can_talk_to_the_daemon_and_a_second_daemon_cannot_take_the_name() {
    let tmp = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let name = pipe_name("first");
    let (stop, handle) = run(options(tmp.path(), &name)).await;

    let pipe = format!("\\\\.\\pipe\\{name}");
    let reply: Value = tokio::task::spawn_blocking({
        let pipe = pipe.clone();
        move || {
            let mut file = OpenOptions::new().read(true).write(true).open(&pipe).unwrap();
            writeln!(
                file,
                "{}",
                json!({"jsonrpc": "2.0", "id": 1, "method": "server.info", "params": {}})
            )
            .unwrap();
            let mut line = String::new();
            BufReader::new(file).read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
    })
    .await
    .unwrap();
    assert_eq!(
        reply["result"]["protocolVersion"],
        misao_protocol::PROTOCOL_VERSION
    );

    // 同じ名前のパイプは、別の data ディレクトリ (pid ファイルで排他できない) からも作れない。
    let second = Daemon::start(options(other.path(), &name)).await;
    assert!(
        matches!(second, Err(DaemonError::AlreadyListening(_))),
        "{:?}",
        second.err()
    );
    let _ = stop.send(());
    handle.await.unwrap();
}

/// SMB 経由 (`\\127.0.0.1\pipe\...`) の接続は、`PIPE_REJECT_REMOTE_CLIENTS` で断る。
/// (SMB のサーバーが動いていない環境でも、接続できないので失敗しない。判別力があるのは SMB が有効なとき。)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_through_the_network_share_path_are_refused() {
    let tmp = TempDir::new().unwrap();
    let name = pipe_name("remote");
    let (stop, handle) = run(options(tmp.path(), &name)).await;
    let remote = format!("\\\\127.0.0.1\\pipe\\{name}");
    let result = tokio::task::spawn_blocking(move || OpenOptions::new().read(true).write(true).open(remote))
        .await
        .unwrap();
    assert!(result.is_err(), "a remote client must not connect");
    let _ = stop.send(());
    handle.await.unwrap();
}
