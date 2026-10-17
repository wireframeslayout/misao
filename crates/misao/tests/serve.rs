//! `misao serve` を実プロセスで起動し、接続口・`server.info`・終了処理を確かめる（一時ディレクトリだけを使う）。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::{Value, json};
use tempfile::TempDir;

fn bin(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_misao"));
    // 常駐の misao や開発者の設定に触れない。
    cmd.env_clear()
        .env("HOME", dir)
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    cmd
}

fn serve(dir: &Path) -> Child {
    let mut child = bin(dir)
        .args(["serve", "--socket"])
        .arg(dir.join("run/misao.sock"))
        .arg("--data")
        .arg(dir.join("run"))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let sock = dir.join("run/misao.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    // 古いソケットファイルが残っていることもあるので、接続できるまで待つ。
    while UnixStream::connect(&sock).is_err() {
        assert!(child.try_wait().unwrap().is_none(), "daemon exited early");
        assert!(Instant::now() < deadline, "daemon did not start listening");
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

fn request(sock: &Path, req: &Value) -> Value {
    let mut stream = UnixStream::connect(sock).unwrap();
    writeln!(stream, "{req}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn terminate(child: &mut Child) -> std::process::ExitStatus {
    kill(Pid::from_raw(child.id().try_into().unwrap()), Signal::SIGTERM).unwrap();
    child.wait().unwrap()
}

#[test]
fn version_flag_prints_name_and_version() {
    let out = bin(Path::new("/")).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("misao {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn serves_server_info_with_secure_modes_and_cleans_up_on_sigterm() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut child = serve(dir);
    let sock = dir.join("run/misao.sock");
    assert_eq!(fs::metadata(dir.join("run")).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&sock).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::read_to_string(dir.join("run/daemon.pid")).unwrap(),
        child.id().to_string()
    );

    let res = request(
        &sock,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server.info", "params": {}}),
    );
    let result = &res["result"];
    assert_eq!(res["id"], 1);
    assert_eq!(result["protocolVersion"], misao_protocol::PROTOCOL_VERSION);
    assert_eq!(result["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(result["pid"], child.id());
    assert_eq!(result["paneCount"], 0);
    assert_eq!(result["eventHead"], 1);
    assert_eq!(result["epoch"].as_str().unwrap().len(), 26);

    let unknown = request(&sock, &json!({"jsonrpc": "2.0", "id": "a", "method": "nope"}));
    assert_eq!(
        unknown["error"]["code"],
        misao_protocol::ErrorCode::METHOD_NOT_FOUND
    );
    let unimplemented = request(&sock, &json!({"jsonrpc": "2.0", "id": 2, "method": "pane.list"}));
    assert_eq!(
        unimplemented["error"]["code"],
        misao_protocol::ErrorCode::NOT_IMPLEMENTED
    );

    assert!(terminate(&mut child).success());
    assert!(!sock.exists() && !dir.join("run/daemon.pid").exists());
}

#[test]
fn second_daemon_on_the_same_socket_fails_and_leaves_the_first_alone() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut first = serve(dir);
    let second = bin(dir)
        .args(["serve", "--socket"])
        .arg(dir.join("run/misao.sock"))
        .arg("--data")
        .arg(dir.join("run2"))
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("[misao] another daemon is already listening on"),
        "{stderr}"
    );
    let res = request(
        &dir.join("run/misao.sock"),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server.info"}),
    );
    assert_eq!(res["result"]["pid"], first.id());
    terminate(&mut first);
}

#[test]
fn pid_file_of_a_live_daemon_blocks_a_second_start() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut first = serve(dir);
    let second = bin(dir)
        .args(["serve", "--socket"])
        .arg(dir.join("other/misao.sock"))
        .arg("--data")
        .arg(dir.join("run"))
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("another daemon (pid"),
        "{second:?}"
    );
    terminate(&mut first);
}

#[test]
fn stale_socket_after_sigkill_is_removed_on_next_start() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut first = serve(dir);
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(
        dir.join("run/misao.sock").exists(),
        "SIGKILL leaves the socket behind"
    );
    let mut again = serve(dir);
    let res = request(
        &dir.join("run/misao.sock"),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server.info"}),
    );
    assert_eq!(res["result"]["pid"], again.id());
    terminate(&mut again);
}

#[test]
fn refuses_a_socket_directory_open_to_group_or_other() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("open");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = bin(tmp.path())
        .args(["serve", "--socket"])
        .arg(dir.join("misao.sock"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("accessible by group/other (mode 755)"));
}

#[test]
fn unparseable_line_gets_a_parse_error_and_the_connection_survives() {
    let tmp = TempDir::new().unwrap();
    let mut child = serve(tmp.path());
    let mut stream = UnixStream::connect(tmp.path().join("run/misao.sock")).unwrap();
    writeln!(stream, "not json").unwrap();
    writeln!(stream, r#"{{"jsonrpc":"2.0","id":9,"method":"server.info"}}"#).unwrap();
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let parsed: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(parsed["error"]["code"], misao_protocol::ErrorCode::PARSE);
    assert_eq!(parsed["id"], Value::Null);
    let mut second = String::new();
    reader.read_line(&mut second).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&second).unwrap()["id"], 9);
    terminate(&mut child);
}

fn wait_exit(child: &mut Child, within: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn sigterm_finishes_even_when_a_client_never_reads_its_responses() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let mut child = serve(dir);
    let mut stream = UnixStream::connect(dir.join("run/misao.sock")).unwrap();
    // server.schema (数十 KB) の応答を読まずに溜めさせ、デーモン側の送信を詰まらせる。
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"server.schema"}"#;
    stream
        .write_all(format!("{req}\n").repeat(600).as_bytes())
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    kill(Pid::from_raw(child.id().try_into().unwrap()), Signal::SIGTERM).unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(5));
    if status.is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert!(
        status.is_some_and(|s| s.success()),
        "daemon did not exit on SIGTERM: {status:?}"
    );
    assert!(!dir.join("run/misao.sock").exists() && !dir.join("run/daemon.pid").exists());
    drop(stream);
}

#[test]
fn concurrent_starts_on_one_socket_leave_exactly_one_daemon_and_keep_its_socket() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let sock = dir.join("run/misao.sock");
    fs::create_dir(dir.join("run")).unwrap();
    fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    // pid ファイルの場所を変えて、ソケットに対する排他だけが効く状況にする。
    let mut children: Vec<Child> = (0..6)
        .map(|i| {
            bin(dir)
                .args(["serve", "--socket"])
                .arg(&sock)
                .arg("--data")
                .arg(dir.join(format!("data{i}")))
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    std::thread::sleep(Duration::from_secs(2));
    let mut alive = Vec::new();
    for c in &mut children {
        if c.try_wait().unwrap().is_none() {
            alive.push(c.id());
        }
    }
    assert_eq!(alive.len(), 1, "exactly one daemon should survive: {alive:?}");
    let res = request(
        &sock,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server.info"}),
    );
    assert_eq!(res["result"]["pid"], alive[0]);
    for c in &mut children {
        if c.try_wait().unwrap().is_none() {
            terminate(c);
        }
    }
}
