//! `misao stop`: 稼働中のデーモンをペインごと止める。Unix は SIGTERM、Windows は停止イベントで伝える（同じ契約）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// `MISAO_*` と設定を持ち込まない `misao`（常駐のデーモンや開発者の設定に触れない）。
fn misao(dir: &Path, endpoint: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_misao"));
    cmd.env_clear().env("MISAO_SOCKET", endpoint);
    // Windows の子プロセスは SystemRoot などが無いと起動できない。Unix は HOME だけでよい。
    for key in [
        "PATH",
        "SystemRoot",
        "SYSTEMROOT",
        "USERPROFILE",
        "USERNAME",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.env("HOME", dir);
    cmd
}

fn endpoint(dir: &Path) -> String {
    if cfg!(windows) {
        format!(
            "\\\\.\\pipe\\misao-test-{}-{}",
            std::process::id(),
            dir.file_name().unwrap().to_string_lossy()
        )
    } else {
        dir.join("run/misao.sock").to_string_lossy().into_owned()
    }
}

fn is_running(dir: &Path, endpoint: &str) -> bool {
    misao(dir, endpoint)
        .arg("status")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn start(dir: &Path, endpoint: &str) -> Child {
    let mut child = misao(dir, endpoint)
        .args(["serve", "--data"])
        .arg(dir.join("run"))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !is_running(dir, endpoint) {
        assert!(child.try_wait().unwrap().is_none(), "the daemon exited early");
        assert!(Instant::now() < deadline, "the daemon did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "the daemon did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn stop_ends_the_daemon_cleanly_and_closes_its_panes() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let endpoint = endpoint(dir);
    let mut daemon = start(dir, &endpoint);

    // ペインを 1 つ持たせておく（止めるときに閉じられる）。
    let command: &[&str] = if cfg!(windows) {
        &["ping", "-n", "300", "127.0.0.1"]
    } else {
        &["sleep", "300"]
    };
    let created = misao(dir, &endpoint)
        .arg("new")
        .arg("--")
        .args(command)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let stopped = misao(dir, &endpoint).args(["stop", "--json"]).output().unwrap();
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&stopped.stdout).unwrap();
    assert_eq!(body["stopped"], true);
    assert_eq!(body["pid"], daemon.id());

    // 正常終了（0）で、接続口は消えている。
    assert!(wait_exit(&mut daemon).success());
    assert!(!is_running(dir, &endpoint));
    assert!(!dir.join("run/daemon.pid").exists(), "the pid file is removed");
}

#[test]
fn stop_without_a_daemon_fails_with_the_unreachable_message() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let output = misao(dir, &endpoint(dir)).arg("stop").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("デーモンに接続できません"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
