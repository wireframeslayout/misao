//! pane.* の RPC と PTY（実際に `sh` を起動して確かめる）。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use common::{Client, connect, core, is_alive, read_file_when_ready, sh, wait_until};

const PANE_NOT_FOUND: i64 = 1001;
const PANE_EXITED: i64 = 1002;
const WINDOW_NOT_FOUND: i64 = 1007;
const INVALID_PARAMS: i64 = -32602;
const NOT_IMPLEMENTED: i64 = 1005;

async fn open(c: &mut Client, cmd: Value, extra: Value) -> String {
    let mut params = json!({"cmd": cmd});
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    c.ok("pane.open", params).await["paneId"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn info(c: &mut Client, pane_id: &str) -> Value {
    c.ok("pane.info", json!({"paneId": pane_id})).await
}

async fn wait_exit(c: &mut Client, pane_id: &str) -> Value {
    let deadline = Instant::now() + common::WAIT;
    loop {
        let i = info(c, pane_id).await;
        if i["processState"] == "exited" {
            return i;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the pane to exit"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn open_creates_the_default_workspace_and_reports_the_pane() {
    let tmp = TempDir::new().unwrap();
    let mut c = connect(&core());
    let pane_id = open(
        &mut c,
        json!(["sleep", "30"]),
        json!({"cwd": tmp.path(), "cols": 100, "rows": 30, "labels": {"owner": "test"}}),
    )
    .await;
    assert!(pane_id.starts_with("p_") && pane_id.len() == 28);
    let i = info(&mut c, &pane_id).await;
    assert_eq!(i["processState"], "running");
    assert!(i["pid"].as_u64().unwrap() > 0);
    assert_eq!(i["cmd"], json!(["sleep", "30"]));
    assert_eq!(i["cwd"], json!(tmp.path()));
    assert_eq!((i["cols"].as_u64(), i["rows"].as_u64()), (Some(100), Some(30)));
    assert_eq!(i["labels"], json!({"owner": "test"}));
    assert_eq!(
        (i["workspace"].as_str(), i["window"]["name"].as_str()),
        (Some("default"), Some("default"))
    );
    assert_eq!(
        (&i["exitCode"], &i["signal"], &i["title"]),
        (&Value::Null, &Value::Null, &json!(""))
    );
    assert_eq!(
        (i["clients"].clone(), i["sizeOwner"].clone()),
        (json!([]), Value::Null)
    );
    let workspaces = c.ok("workspace.list", json!({})).await;
    assert_eq!(workspaces[0]["name"], "default");
    assert_eq!(workspaces[0]["windows"][0]["name"], "default");
    assert_eq!(c.ok("server.info", json!({})).await["paneCount"], 1);
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    assert_eq!(c.ok("server.info", json!({})).await["paneCount"], 0);
}

#[tokio::test]
async fn exit_code_and_signal_are_reported_like_the_node_daemon() {
    let mut c = connect(&core());
    let exited = open(&mut c, sh("exit 3"), json!({})).await;
    let i = wait_exit(&mut c, &exited).await;
    assert_eq!((&i["exitCode"], &i["signal"]), (&json!(3), &Value::Null));
    assert_eq!(
        (&i["agentState"], &i["decidedBy"]),
        (&json!("exited"), &json!("exit"))
    );

    let killed = open(&mut c, sh("kill -TERM $$"), json!({})).await;
    let i = wait_exit(&mut c, &killed).await;
    assert_eq!((&i["exitCode"], &i["signal"]), (&Value::Null, &json!(15)));
    assert_eq!(
        c.err_code("pane.write", json!({"paneId": killed, "data": "x"}))
            .await,
        PANE_EXITED
    );
}

#[tokio::test]
async fn cwd_and_env_reach_the_child_and_ephemeral_env_wins_without_being_reported() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    let script = format!(
        "pwd > {out}; echo \"$A|$B|$MISAO_PANE_ID|$TERM|$COLORTERM|$TMUX|$HOME\" >> {out}",
        out = out.display()
    );
    let mut c = connect(&core());
    let pane_id = open(
        &mut c,
        sh(&script),
        json!({"cwd": tmp.path(), "env": {"A": "env-a", "B": "env-b"}, "ephemeralEnv": {"B": "secret-b"}}),
    )
    .await;
    let text = read_file_when_ready(&out).await;
    let mut lines = text.lines();
    let cwd = std::fs::canonicalize(lines.next().unwrap()).unwrap();
    assert_eq!(cwd, std::fs::canonicalize(tmp.path()).unwrap());
    let home = std::env::var("HOME").unwrap();
    assert_eq!(
        lines.next(),
        Some(format!("env-a|secret-b|{pane_id}|xterm-256color|truecolor||{home}").as_str())
    );
    let reported = serde_json::to_string(&info(&mut c, &pane_id).await).unwrap();
    assert!(
        !reported.contains("secret-b"),
        "ephemeralEnv must not be reported: {reported}"
    );
}

#[tokio::test]
async fn pty_size_follows_open_and_resize() {
    let tmp = TempDir::new().unwrap();
    let (first, second) = (tmp.path().join("first"), tmp.path().join("second"));
    let script = format!(
        "stty size > {}; read x; stty size > {}",
        first.display(),
        second.display()
    );
    let mut c = connect(&core());
    let pane_id = open(&mut c, sh(&script), json!({"cols": 100, "rows": 30})).await;
    assert_eq!(read_file_when_ready(&first).await, "30 100");
    c.ok(
        "pane.resize",
        json!({"paneId": pane_id, "cols": 50, "rows": 20, "clientId": "A"}),
    )
    .await;
    let i = info(&mut c, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["rows"].as_u64(), i["sizeOwner"].as_str()),
        (Some(50), Some(20), Some("A"))
    );
    c.ok("pane.write", json!({"paneId": pane_id, "data": "\n"})).await;
    assert_eq!(read_file_when_ready(&second).await, "20 50");
}

#[tokio::test]
async fn write_sends_text_and_base64_input_and_claims_the_size_for_the_writer() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    let script = format!("read a; read b; echo \"$a/$b\" > {}", out.display());
    let mut c = connect(&core());
    let pane_id = open(&mut c, sh(&script), json!({"cols": 80, "rows": 24})).await;
    c.ok(
        "pane.resize",
        json!({"paneId": pane_id, "cols": 120, "rows": 40, "clientId": "A"}),
    )
    .await;
    c.ok(
        "pane.resize",
        json!({"paneId": pane_id, "cols": 60, "rows": 20, "clientId": "B"}),
    )
    .await;
    assert_eq!(info(&mut c, &pane_id).await["sizeOwner"], "B");
    // A が入力すると、A のサイズが所有者になる
    c.ok(
        "pane.write",
        json!({"paneId": pane_id, "data": "one\n", "clientId": "A", "source": "terminal"}),
    )
    .await;
    let i = info(&mut c, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["rows"].as_u64(), i["sizeOwner"].as_str()),
        (Some(120), Some(40), Some("A"))
    );
    // "two\n" の base64
    c.ok("pane.write", json!({"paneId": pane_id, "dataB64": "dHdvCg=="}))
        .await;
    assert_eq!(read_file_when_ready(&out).await, "one/two");
    assert_eq!(
        c.err_code(
            "pane.write",
            json!({"paneId": pane_id, "data": "x", "dataB64": "eA=="})
        )
        .await,
        INVALID_PARAMS
    );
    assert_eq!(
        c.err_code("pane.write", json!({"paneId": pane_id})).await,
        INVALID_PARAMS
    );
}

#[tokio::test]
async fn list_filters_by_state_labels_and_workspace() {
    let mut c = connect(&core());
    c.ok("workspace.create", json!({"name": "other"})).await;
    let w = c
        .ok("window.create", json!({"workspace": "other", "name": "w"}))
        .await;
    let running = open(
        &mut c,
        json!(["sleep", "30"]),
        json!({"labels": {"k": "v", "x": "1"}}),
    )
    .await;
    let in_other = open(&mut c, json!(["sleep", "30"]), json!({"windowId": w["windowId"]})).await;
    let exited = open(&mut c, sh("exit 0"), json!({"labels": {"k": "v"}})).await;
    wait_exit(&mut c, &exited).await;

    let ids = |v: Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|p| p["paneId"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ids(c.ok("pane.list", json!({})).await),
        [running.clone(), in_other.clone(), exited.clone()]
    );
    assert_eq!(
        ids(c.ok("pane.list", json!({"filter": {"state": "running"}})).await),
        [running.clone(), in_other.clone()]
    );
    assert_eq!(
        ids(c.ok("pane.list", json!({"filter": {"state": "exited"}})).await),
        vec![exited.clone()]
    );
    assert!(ids(c.ok("pane.list", json!({"filter": {"state": "stopped"}})).await).is_empty());
    assert_eq!(
        ids(c.ok("pane.list", json!({"filter": {"labels": {"k": "v"}}})).await),
        [running.clone(), exited.clone()]
    );
    assert_eq!(
        ids(c
            .ok("pane.list", json!({"filter": {"labels": {"k": "v", "x": "1"}}}))
            .await),
        vec![running.clone()]
    );
    assert_eq!(
        ids(c.ok("pane.list", json!({"filter": {"workspace": "other"}})).await),
        vec![in_other.clone()]
    );
    assert!(ids(c.ok("pane.list", json!({"filter": {"workspace": "none"}})).await).is_empty());
    assert_eq!(
        c.err_code("pane.list", json!({"filter": {"unknown": 1}})).await,
        INVALID_PARAMS
    );
}

#[tokio::test]
async fn set_label_applies_set_then_unset() {
    let mut c = connect(&core());
    let pane_id = open(
        &mut c,
        json!(["sleep", "30"]),
        json!({"labels": {"a": "1", "b": "2"}}),
    )
    .await;
    let r = c
        .ok(
            "pane.set_label",
            json!({"paneId": pane_id, "set": {"c": "3", "a": "9"}, "unset": ["b"]}),
        )
        .await;
    assert_eq!(r, json!({"labels": {"a": "9", "c": "3"}}));
    assert_eq!(
        info(&mut c, &pane_id).await["labels"],
        json!({"a": "9", "c": "3"})
    );
    assert_eq!(
        c.err_code("pane.set_label", json!({"paneId": pane_id})).await,
        INVALID_PARAMS
    );
}

#[tokio::test]
async fn open_failures_use_codes_and_leave_nothing_behind() {
    let mut c = connect(&core());
    assert_eq!(
        c.err_code("pane.open", json!({"cmd": ["/nonexistent/command"]}))
            .await,
        INVALID_PARAMS
    );
    assert_eq!(
        c.err_code(
            "pane.open",
            json!({"cmd": ["sleep", "1"], "cwd": "/nonexistent/dir"})
        )
        .await,
        INVALID_PARAMS
    );
    assert_eq!(c.err_code("pane.open", json!({"cmd": []})).await, INVALID_PARAMS);
    let missing = format!("w_{}", "0".repeat(26));
    assert_eq!(
        c.err_code("pane.open", json!({"cmd": ["sleep", "1"], "windowId": missing}))
            .await,
        WINDOW_NOT_FOUND
    );
    let preplace = json!([{"path": "a", "content": "b"}]);
    assert_eq!(
        c.err_code("pane.open", json!({"cmd": ["sleep", "1"], "preplace": preplace}))
            .await,
        NOT_IMPLEMENTED
    );
    // 既定の workspace / window も、pane も残らない。
    assert_eq!(c.ok("workspace.list", json!({})).await, json!([]));
    assert_eq!(c.ok("pane.list", json!({})).await, json!([]));
    let unknown = format!("p_{}", "0".repeat(26));
    for method in ["pane.info", "pane.close"] {
        assert_eq!(
            c.err_code(method, json!({"paneId": unknown})).await,
            PANE_NOT_FOUND,
            "{method}"
        );
    }
    assert_eq!(
        c.err_code("pane.resize", json!({"paneId": unknown, "cols": 1, "rows": 1}))
            .await,
        PANE_NOT_FOUND
    );
    assert_eq!(
        c.err_code("pane.send_keys", json!({"paneId": unknown, "keys": ["Enter"]}))
            .await,
        NOT_IMPLEMENTED
    );
}

/// `sh -c` でバックグラウンドの子を起動し、その pid をファイルに書かせる。
/// `background` は `( ... )` の中身で、`ready` が済むまで（trap などの設定が終わるまで）pid を書かない。
async fn spawn_with_background_child(c: &mut Client, tmp: &TempDir, background: &str) -> (String, String) {
    let (pid_file, ready) = (tmp.path().join("child.pid"), tmp.path().join("ready"));
    let script = format!(
        "({background}; touch {ready}; exec sleep 300) & pid=$!; while [ ! -e {ready} ]; do sleep 0.05; done; echo $pid > {pid_file}; wait",
        ready = ready.display(),
        pid_file = pid_file.display()
    );
    let pane_id = open(c, sh(&script), json!({})).await;
    (pane_id, read_file_when_ready(&pid_file).await)
}

#[tokio::test]
async fn close_stops_the_whole_process_group() {
    let tmp = TempDir::new().unwrap();
    let mut c = connect(&core());
    let (pane_id, child) = spawn_with_background_child(&mut c, &tmp, ":").await;
    assert!(is_alive(&child));
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    wait_until("the background child to die", || async { !is_alive(&child) }).await;
    assert_eq!(
        c.err_code("pane.info", json!({"paneId": pane_id})).await,
        PANE_NOT_FOUND
    );
}

#[tokio::test]
async fn close_kills_a_child_that_ignores_sighup_after_the_grace_period() {
    let tmp = TempDir::new().unwrap();
    let mut c = connect(&core());
    // リーダー (sh) は SIGHUP で終わるが、背景の子は SIGHUP を無視する。
    let (pane_id, child) = spawn_with_background_child(&mut c, &tmp, "trap '' HUP").await;
    let started = Instant::now();
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    assert!(
        !is_alive(&child),
        "close must not return while a child of the pane is still running"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "the survivor gets a grace period first"
    );
}

#[tokio::test]
async fn close_kills_a_leader_that_ignores_sighup_after_the_grace_period() {
    let tmp = TempDir::new().unwrap();
    let ready = tmp.path().join("ready");
    let mut c = connect(&core());
    // trap を設定し終えてから close する（起動直後の SIGHUP は trap より先に届くことがある）。
    let script = format!("trap '' HUP; touch {}; sleep 300 & wait", ready.display());
    let pane_id = open(&mut c, sh(&script), json!({})).await;
    wait_until("the trap to be set", || async { ready.exists() }).await;
    let pid = info(&mut c, &pane_id).await["pid"].as_u64().unwrap().to_string();
    let started = Instant::now();
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    assert!(!is_alive(&pid));
    assert!(started.elapsed() >= Duration::from_secs(2));
}

#[tokio::test]
async fn closing_a_window_or_workspace_closes_the_panes_inside() {
    let tmp = TempDir::new().unwrap();
    let mut c = connect(&core());
    let (pane_id, child) = spawn_with_background_child(&mut c, &tmp, ":").await;
    let window_id = info(&mut c, &pane_id).await["window"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    c.ok("window.close", json!({"windowId": window_id})).await;
    assert_eq!(c.ok("pane.list", json!({})).await, json!([]));
    wait_until("the child to die", || async { !is_alive(&child) }).await;

    c.ok("workspace.create", json!({"name": "ws"})).await;
    let w = c
        .ok("window.create", json!({"workspace": "ws", "name": "w"}))
        .await;
    let a = open(
        &mut c,
        json!(["sleep", "300"]),
        json!({"windowId": w["windowId"]}),
    )
    .await;
    let b = open(
        &mut c,
        json!(["sleep", "300"]),
        json!({"windowId": w["windowId"]}),
    )
    .await;
    c.ok("workspace.close", json!({"name": "ws"})).await;
    for id in [a, b] {
        assert_eq!(
            c.err_code("pane.info", json!({"paneId": id})).await,
            PANE_NOT_FOUND
        );
    }
}

#[tokio::test]
async fn shutdown_closes_every_pane_without_leaving_children() {
    let tmp = TempDir::new().unwrap();
    let core = core();
    let mut c = connect(&core);
    let (_, child) = spawn_with_background_child(&mut c, &tmp, ":").await;
    core.shutdown().await;
    assert!(!is_alive(&child));
}

#[tokio::test]
async fn sizes_beyond_16_bits_are_reported_as_given_and_only_clamped_for_the_pty() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    let mut c = connect(&core());
    let script = format!("read x; stty size > {}", out.display());
    let pane_id = open(&mut c, sh(&script), json!({"cols": 100_000, "rows": 70_000})).await;
    let i = info(&mut c, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["rows"].as_u64()),
        (Some(100_000), Some(70_000))
    );
    c.ok(
        "pane.resize",
        json!({"paneId": pane_id, "cols": 9_007_199_254_740_991u64, "rows": 3}),
    )
    .await;
    let i = info(&mut c, &pane_id).await;
    assert_eq!(i["cols"].as_u64(), Some(9_007_199_254_740_991));
    c.ok("pane.write", json!({"paneId": pane_id, "data": "\n"})).await;
    assert_eq!(read_file_when_ready(&out).await, "3 65535");
    assert_eq!(
        c.err_code(
            "pane.resize",
            json!({"paneId": pane_id, "cols": 9_007_199_254_740_992u64, "rows": 3})
        )
        .await,
        INVALID_PARAMS
    );
}

/// リーダーだけが自然に終了し、SIGHUP を無視する子がグループに残った pane を作る。
async fn pane_whose_leader_exited_leaving_a_survivor(c: &mut Client, tmp: &TempDir) -> (String, String) {
    let (pid_file, ready) = (tmp.path().join("survivor.pid"), tmp.path().join("ready"));
    let script = format!(
        "(trap '' HUP; touch {ready}; exec sleep 300) & pid=$!; while [ ! -e {ready} ]; do sleep 0.05; done; echo $pid > {pid_file}",
        ready = ready.display(),
        pid_file = pid_file.display()
    );
    let pane_id = open(c, sh(&script), json!({})).await;
    let survivor = read_file_when_ready(&pid_file).await;
    let i = wait_exit(c, &pane_id).await;
    assert_eq!(i["exitCode"], 0);
    assert!(is_alive(&survivor), "the survivor outlives the leader");
    (pane_id, survivor)
}

#[tokio::test]
async fn closing_a_pane_whose_leader_already_exited_still_stops_the_surviving_group() {
    let tmp = TempDir::new().unwrap();
    let mut c = connect(&core());
    let (pane_id, survivor) = pane_whose_leader_exited_leaving_a_survivor(&mut c, &tmp).await;
    let started = Instant::now();
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    assert!(
        !is_alive(&survivor),
        "close must not return while a child of the pane is still running"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "SIGHUP first, SIGKILL after the grace period"
    );
    assert_eq!(
        c.err_code("pane.info", json!({"paneId": pane_id})).await,
        PANE_NOT_FOUND
    );
}

#[tokio::test]
async fn shutdown_stops_the_surviving_group_of_an_exited_pane() {
    let tmp = TempDir::new().unwrap();
    let core = core();
    let mut c = connect(&core);
    let (_, survivor) = pane_whose_leader_exited_leaving_a_survivor(&mut c, &tmp).await;
    core.shutdown().await;
    assert!(!is_alive(&survivor));
}
