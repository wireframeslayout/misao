//! `misao` の CLI を実プロセスで動かし、`misao serve` のデーモンに対して出力形式と終了コードを確かめる
//! （一時ディレクトリだけを使う。Node 版との突き合わせは `packages/cli/test` を `MISAO_CLI_BIN` で動かして行う）。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::Value;
use tempfile::TempDir;

struct Daemon {
    dir: TempDir,
    socket: PathBuf,
    child: Child,
}

struct Output {
    code: i32,
    out: String,
    err: String,
}

fn misao(dir: &Path, socket: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_misao"));
    // 常駐の misao や開発者の設定に触れない。
    cmd.env_clear()
        .env("HOME", dir)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("MISAO_SOCKET", socket)
        .current_dir(dir)
        .args(args);
    cmd
}

impl Daemon {
    fn start() -> Self {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("run/misao.sock");
        let child = misao(dir.path(), &socket, &["serve", "--data"])
            .arg(dir.path().join("run"))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "the daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { dir, socket, child }
    }

    fn command(&self, args: &[&str]) -> Command {
        misao(self.dir.path(), &self.socket, args)
    }

    fn run(&self, args: &[&str]) -> Output {
        let out = self.command(args).stdin(Stdio::null()).output().unwrap();
        Output {
            code: out.status.code().unwrap(),
            out: String::from_utf8(out.stdout).unwrap(),
            err: String::from_utf8(out.stderr).unwrap(),
        }
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert_eq!(out.code, 0, "{args:?}: {}", out.err);
        serde_json::from_str(&out.out).unwrap()
    }

    fn wait_screen(&self, target: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.run(&["screen", target]).out.contains(needle) {
            assert!(
                Instant::now() < deadline,
                "screen of {target} never showed {needle}"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = kill(
            Pid::from_raw(self.child.id().try_into().unwrap()),
            Signal::SIGTERM,
        );
        let _ = self.child.wait();
    }
}

const READ_ONE: &str = "echo ready-marker; read x; echo got:$x; sleep 60";

fn new_pane(d: &Daemon, name: &str, script: &str) -> Value {
    d.json(&[
        "new",
        "--json",
        "--label",
        &format!("name={name}"),
        "--",
        "sh",
        "-c",
        script,
    ])
}

#[test]
fn without_a_daemon_status_says_stopped_and_other_commands_explain_how_to_start_it() {
    let dir = TempDir::new().unwrap();
    let socket = dir.path().join("none/misao.sock");
    let run = |args: &[&str]| {
        let o = misao(dir.path(), &socket, args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            o.status.code().unwrap(),
            String::from_utf8(o.stdout).unwrap(),
            String::from_utf8(o.stderr).unwrap(),
        )
    };
    let (code, out, _) = run(&["status"]);
    assert_eq!(code, 1);
    assert!(out.contains("停止中"), "{out}");
    let (code, out, _) = run(&["status", "--json"]);
    assert_eq!(
        (
            code,
            serde_json::from_str::<Value>(&out).unwrap()["running"].clone()
        ),
        (1, Value::Bool(false))
    );
    let (code, _, err) = run(&["ls"]);
    assert_eq!(code, 1);
    assert!(err.contains("misao serve"), "{err}");
    let (code, _, err) = run(&["ls", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&err).unwrap()["error"]["code"],
        "daemon_unreachable"
    );
}

#[test]
fn arguments_help_and_version() {
    let dir = TempDir::new().unwrap();
    let socket = dir.path().join("none/misao.sock");
    let run = |args: &[&str]| {
        let o = misao(dir.path(), &socket, args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            o.status.code().unwrap(),
            String::from_utf8(o.stdout).unwrap(),
            String::from_utf8(o.stderr).unwrap(),
        )
    };
    assert_eq!(
        run(&["--version"]),
        (0, format!("misao {}\n", env!("CARGO_PKG_VERSION")), String::new())
    );
    let (code, _, err) = run(&[]);
    assert_eq!(code, 2);
    assert!(err.contains("コマンドを指定してください"));
    let (code, out, _) = run(&["--help"]);
    assert_eq!(code, 0);
    for name in [
        "ls", "attach", "new", "kill", "send", "screen", "tail", "events", "label", "status", "schema",
        "serve",
    ] {
        assert!(
            out.lines().any(|l| l.starts_with(&format!("  {name} "))),
            "{name}"
        );
    }
    assert!(out.contains("数字だけの ID は p_ を付けて書く"));
    assert!(run(&["send", "--help"]).1.contains("対象 <target> の書き方:"));
    assert!(!run(&["status", "--help"]).1.contains("対象 <target>"));
    assert_eq!(run(&["frobnicate"]).0, 2);
    let (code, _, err) = run(&["ls", "--nope", "--json"]);
    assert_eq!(code, 2);
    assert_eq!(
        serde_json::from_str::<Value>(&err).unwrap()["error"]["code"],
        "usage"
    );
    assert_eq!(run(&["status", "extra"]).0, 2);
}

#[test]
fn status_schema_and_the_pane_commands_work_against_a_daemon() {
    let d = Daemon::start();
    let human = d.run(&["status"]);
    assert_eq!(human.code, 0);
    assert!(
        human.out.contains("稼働中") && human.out.contains(d.socket.to_str().unwrap()),
        "{}",
        human.out
    );
    let status = d.json(&["status", "--json"]);
    assert_eq!(status["running"], true);
    assert_eq!(status["pid"], d.child.id());
    assert!(d.json(&["schema"]).get("methods").is_some());

    let created = d.run(&[
        "new",
        "--label",
        "name=通しテスト",
        "--label",
        "windowId=901",
        "--cwd",
        d.dir.path().to_str().unwrap(),
        "--",
        "sh",
        "-c",
        READ_ONE,
    ]);
    assert_eq!(created.code, 0, "{}", created.err);
    let lines: Vec<&str> = created.out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", created.out);
    assert!(
        lines[0].starts_with("[misao] ペイン p_") && lines[0].contains("を作成しました（sh · "),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains("AZITO の Objects に「未登録」として表示されます"));
    let suffix = lines[2].rsplit(' ').next().unwrap();
    assert!(lines[2].contains("入る: misao attach "));

    let ls = d.run(&["ls"]);
    assert!(ls.out.lines().next().unwrap().starts_with("STATE"), "{}", ls.out);
    assert!(ls.out.contains("W-901 · 通しテスト"));
    let panes = d.json(&["ls", "--json", "--workspace", "default"]);
    assert_eq!(panes.as_array().unwrap().len(), 1);
    assert_eq!(panes[0]["labels"]["origin"], "terminal");
    assert_eq!(
        d.json(&["ls", "--json", "--workspace", "nope"]),
        Value::Array(Vec::new())
    );
    assert_eq!(d.run(&["ls", "--state", "bogus"]).code, 2);

    let label = d.run(&["label", "W-901", "owner=me", "--unset", "name"]);
    assert!(
        label.out.lines().any(|l| l == "owner=me") && !label.out.contains("name="),
        "{}",
        label.out
    );
    assert_eq!(d.run(&["label", "901"]).code, 2);
    assert_eq!(d.run(&["label", "901", "novalue"]).code, 2);

    d.wait_screen("901", "ready-marker");
    assert_eq!(d.run(&["send", "901", "hello", "--enter"]).code, 0);
    d.wait_screen("901", "got:hello");
    let last = d.run(&["screen", "901", "--lines", "1"]);
    assert_eq!(last.out.trim().lines().count(), 1);
    assert!(
        d.json(&["screen", "901", "--json"])["text"]
            .as_str()
            .unwrap()
            .contains("got:hello")
    );
    assert_eq!(d.run(&["screen", "901", "--lines", "0"]).code, 2);

    let sent = d.json(&["send", suffix, "--keys", "C-c", "--json"]);
    assert_eq!(sent["bytes"], 1);
    assert_eq!(d.run(&["send", suffix]).code, 2);
    assert_eq!(d.run(&["send", suffix, "--keys", "NoSuchKey"]).code, 2);
    assert_eq!(d.run(&["kill", suffix]).code, 2, "no terminal and no --force");
    let killed = d.json(&["kill", suffix, "--force", "--json"]);
    assert_eq!(killed["closed"], "pane");
    assert_eq!(d.json(&["ls", "--json"]), Value::Array(Vec::new()));
}

#[test]
fn ambiguous_and_missing_targets_exit_with_one() {
    let d = Daemon::start();
    let a = new_pane(&d, "twin one", "sleep 60");
    let b = new_pane(&d, "twin two", "sleep 60");
    let ambiguous = d.run(&["screen", "twin"]);
    assert_eq!(ambiguous.code, 1);
    for pane in [&a, &b] {
        assert!(
            ambiguous.err.contains(pane["paneId"].as_str().unwrap()),
            "{}",
            ambiguous.err
        );
    }
    let json: Value = serde_json::from_str(&d.run(&["screen", "twin", "--json"]).err).unwrap();
    assert_eq!(json["error"]["code"], "ambiguous");
    assert_eq!(json["error"]["candidates"].as_array().unwrap().len(), 2);
    let missing = d.run(&["screen", "nothing-matches"]);
    assert_eq!(missing.code, 1);
    assert!(missing.err.contains("当てはまるペインがありません"));
}

#[test]
fn new_in_a_workspace_and_window_and_kill_by_scope() {
    let d = Daemon::start();
    let make = |window: &str| {
        d.json(&[
            "new",
            "--json",
            "--workspace",
            "ws-a",
            "--window",
            window,
            "--",
            "sleep",
            "60",
        ])
    };
    let first = make("win-a");
    let second = make("win-a");
    assert_eq!(first["window"]["id"], second["window"]["id"]);
    assert_eq!(
        (first["workspace"].as_str(), first["window"]["name"].as_str()),
        (Some("ws-a"), Some("win-a"))
    );
    let other = make("win-b");
    assert_ne!(first["window"]["id"], other["window"]["id"]);
    let by_window = d.json(&[
        "kill",
        first["paneId"].as_str().unwrap(),
        "--window",
        "--force",
        "--json",
    ]);
    assert_eq!(by_window["paneIds"].as_array().unwrap().len(), 2);
    assert_eq!(
        d.json(&["ls", "--json", "--workspace", "ws-a"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    d.json(&[
        "kill",
        other["paneId"].as_str().unwrap(),
        "--workspace",
        "--force",
        "--json",
    ]);
    assert_eq!(
        d.json(&["ls", "--json", "--workspace", "ws-a"]),
        Value::Array(Vec::new())
    );
}

fn read_until(reader: &mut BufReader<impl Read>, count: usize) -> Vec<String> {
    (0..count)
        .map(|_| {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            line.trim_end().to_string()
        })
        .collect()
}

#[test]
fn tail_replays_then_follows_and_exits_zero_on_sigint() {
    let d = Daemon::start();
    let pane = new_pane(&d, "tail-me", "echo first; sleep 0.5; echo second; sleep 60");
    let id = pane["paneId"].as_str().unwrap();
    let mut child = d
        .command(&["tail", "tail-me"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    assert_eq!(read_until(&mut reader, 2), vec!["first", "second"]);
    kill(Pid::from_raw(child.id().try_into().unwrap()), Signal::SIGINT).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert_eq!(err, "");

    let epoch = d.json(&["status", "--json"])["epoch"]
        .as_str()
        .unwrap()
        .to_string();
    let mut json = d
        .command(&["tail", id, "--since", "1", "--epoch", &epoch, "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(json.stdout.take().unwrap());
    let line: Value = serde_json::from_str(&read_until(&mut reader, 1)[0]).unwrap();
    assert_eq!(
        (
            line["seq"].as_u64(),
            line["text"].as_str(),
            line["epoch"].as_str()
        ),
        (Some(2), Some("second"), Some(epoch.as_str()))
    );
    kill(Pid::from_raw(json.id().try_into().unwrap()), Signal::SIGINT).unwrap();
    assert_eq!(json.wait().unwrap().code(), Some(0));
    assert_eq!(
        d.run(&["tail", id, "--epoch", &epoch]).code,
        2,
        "--epoch needs --since"
    );
    assert_eq!(d.run(&["tail", "nothing"]).code, 1);
}

#[test]
fn events_follow_live_events_and_filter_by_pane() {
    let d = Daemon::start();
    let a = new_pane(&d, "ev-a", "sleep 60");
    let b = new_pane(&d, "ev-b", "sleep 60");
    let (a_id, b_id) = (a["paneId"].as_str().unwrap(), b["paneId"].as_str().unwrap());
    let spawn = |extra: &[&str]| {
        let mut args = vec!["events", "--json"];
        args.extend_from_slice(extra);
        d.command(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut all = spawn(&[]);
    let mut only = spawn(&["--pane", "ev-a"]);
    std::thread::sleep(Duration::from_millis(500)); // 購読の確立を待つ
    d.json(&["label", b_id, "who=b", "--json"]);
    d.json(&["label", a_id, "who=a", "--json"]);
    let mut all_out = BufReader::new(all.stdout.take().unwrap());
    let mut only_out = BufReader::new(only.stdout.take().unwrap());
    let parse = |line: &str| serde_json::from_str::<Value>(line).unwrap();
    let got: Vec<Value> = read_until(&mut all_out, 2).iter().map(|l| parse(l)).collect();
    assert_eq!(
        got.iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["pane.label", "pane.label"]
    );
    assert_eq!(
        got.iter()
            .map(|e| e["paneId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![b_id, a_id]
    );
    let only_events = parse(&read_until(&mut only_out, 1)[0]);
    assert_eq!(only_events["paneId"], a_id);
    assert!(
        got.iter().all(|e| e["epoch"].is_string()),
        "every --json line carries the epoch"
    );
    for child in [&mut all, &mut only] {
        kill(Pid::from_raw(child.id().try_into().unwrap()), Signal::SIGINT).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(0));
    }
    let replay = d
        .command(&["events", "--since", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut replay = replay;
    let mut out = BufReader::new(replay.stdout.take().unwrap());
    let first = read_until(&mut out, 1).remove(0);
    assert!(
        first.starts_with("1  ") && first.contains("  daemon.started  -  {"),
        "{first}"
    );
    kill(Pid::from_raw(replay.id().try_into().unwrap()), Signal::SIGINT).unwrap();
    replay.wait().unwrap();
}

#[test]
fn send_reads_stdin_and_attach_without_a_terminal_is_a_usage_error() {
    let d = Daemon::start();
    new_pane(&d, "stdin-pane", "read a; echo A:$a; sleep 60");
    let mut child = d
        .command(&["send", "stdin-pane", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"two\n").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    d.wait_screen("stdin-pane", "A:two");
    let attach = d.run(&["attach", "stdin-pane"]);
    assert_eq!(attach.code, 2);
    assert!(attach.err.contains("TTY"), "{}", attach.err);
    assert_eq!(
        d.run(&["new", "--attach"]).code,
        2,
        "no pane is created without a terminal"
    );
    assert_eq!(d.json(&["ls", "--json"]).as_array().unwrap().len(), 1);
}
