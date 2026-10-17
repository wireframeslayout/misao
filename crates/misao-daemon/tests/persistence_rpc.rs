//! 状態の保存と復元（`persistence.json`）。再起動後の pane は `stopped` として残る。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use misao_daemon::{Core, CoreOptions, Limits, LogLevel, Logger, builtin_profiles};
use serde_json::{Value, json};
use tempfile::TempDir;

use common::{connect, core_at, sh, wait_until};

const PANE_NOT_FOUND: i64 = 1001;
const PANE_EXITED: i64 = 1002;
const INTERNAL: i64 = -32603;

fn read_state(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn panes_come_back_as_stopped_after_a_restart_and_can_be_edited_and_closed() {
    let tmp = TempDir::new().unwrap();
    let state_path = tmp.path().join("persistence.json");
    let first = core_at(&state_path);
    let mut c = connect(&first);
    c.ok("workspace.create", json!({"name": "dev"})).await;
    let window = c
        .ok("window.create", json!({"workspace": "dev", "name": "main"}))
        .await;
    let window_id = window["windowId"].as_str().unwrap().to_string();
    let pane_id = c
        .ok(
            "pane.open",
            json!({
                "cmd": ["sleep", "30"], "cwd": tmp.path(), "windowId": window_id, "cols": 90, "rows": 30,
                "env": {"VISIBLE": "yes"}, "ephemeralEnv": {"SECRET_TOKEN": "s3cret"}, "labels": {"task": "7"},
            }),
        )
        .await["paneId"]
        .as_str()
        .unwrap()
        .to_string();

    // 保存した内容: env は含むが ephemeralEnv は含まない。Node 版と同じ形（version / workspaces / panes）。
    let saved = read_state(&state_path);
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["workspaces"][0]["name"], "dev");
    assert_eq!(saved["panes"][0]["paneId"], json!(pane_id));
    assert_eq!(saved["panes"][0]["env"], json!({"VISIBLE": "yes"}));
    assert_eq!(saved["panes"][0]["labels"], json!({"task": "7"}));
    assert!(!fs::read_to_string(&state_path).unwrap().contains("s3cret"));

    first.shutdown().await;
    drop(c);
    drop(first);

    let second = core_at(&state_path);
    let mut c = connect(&second);
    let info = c.ok("pane.info", json!({"paneId": pane_id})).await;
    assert_eq!(info["processState"], "stopped");
    assert_eq!(
        (&info["pid"], &info["exitCode"], &info["signal"], &info["title"]),
        (&Value::Null, &Value::Null, &Value::Null, &json!(""))
    );
    assert_eq!(
        (&info["agentState"], &info["decidedBy"]),
        (&json!("unknown"), &json!("none"))
    );
    assert_eq!(info["cmd"], json!(["sleep", "30"]));
    assert_eq!(
        (info["cols"].as_u64(), info["rows"].as_u64()),
        (Some(90), Some(30))
    );
    assert_eq!(info["labels"], json!({"task": "7"}));
    assert_eq!(
        (info["workspace"].as_str(), info["window"]["name"].as_str()),
        (Some("dev"), Some("main"))
    );
    assert_eq!(info["clients"], json!([]));
    let listed = c.ok("pane.list", json!({"filter": {"state": "stopped"}})).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(
        c.ok("pane.list", json!({"filter": {"state": "running"}})).await,
        json!([])
    );
    assert_eq!(c.ok("server.info", json!({})).await["paneCount"], 1);

    // 実行体が無いので入出力はできない
    for (method, params) in [
        ("pane.write", json!({"paneId": pane_id, "data": "x"})),
        ("pane.resize", json!({"paneId": pane_id, "cols": 80, "rows": 24})),
        ("pane.screen", json!({"paneId": pane_id})),
        ("pane.attach", json!({"paneId": pane_id, "clientId": "A"})),
        ("pane.subscribe_lines", json!({"paneId": pane_id})),
    ] {
        assert_eq!(c.err_code(method, params).await, PANE_EXITED, "{method}");
    }

    // ラベルは編集でき、保存される。ID は変わらない。
    let labels = c
        .ok(
            "pane.set_label",
            json!({"paneId": pane_id, "set": {"owner": "x"}, "unset": ["task"]}),
        )
        .await;
    assert_eq!(labels["labels"], json!({"owner": "x"}));
    assert_eq!(
        read_state(&state_path)["panes"][0]["labels"],
        json!({"owner": "x"})
    );

    // 閉じると記録が消え、pane.closed が出る。
    let mut events = connect(&second);
    events.ok("events.subscribe", json!({})).await;
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    assert_eq!(read_state(&state_path)["panes"], json!([]));
    assert_eq!(
        c.err_code("pane.info", json!({"paneId": pane_id})).await,
        PANE_NOT_FOUND
    );
    loop {
        let n = events.next_notification().await;
        if n["params"]["type"] == "pane.closed" {
            assert_eq!(n["params"]["paneId"], json!(pane_id));
            break;
        }
    }
}

#[tokio::test]
async fn a_file_written_by_the_node_daemon_is_read_and_closing_a_window_removes_stopped_panes() {
    let tmp = TempDir::new().unwrap();
    let state_path = tmp.path().join("persistence.json");
    let win = "w_01M3AAAAAAAAAAAAAAAAAAAAAA";
    let pane = "p_01M3AAAAAAAAAAAAAAAAAAAAAA";
    fs::write(
        &state_path,
        format!(
            r#"{{
  "version": 1,
  "workspaces": [{{"name": "default", "windows": [{{"id": "{win}", "name": "default"}}]}}],
  "panes": [{{"paneId": "{pane}", "windowId": "{win}", "cmd": ["bash", "-l"], "cwd": "/tmp",
             "env": {{}}, "labels": {{"name": "shell"}}, "cols": 100, "rows": 40}}]
}}
"#
        ),
    )
    .unwrap();
    let mut c = connect(&core_at(&state_path));
    assert_eq!(
        c.ok("pane.info", json!({"paneId": pane})).await["labels"],
        json!({"name": "shell"})
    );
    c.ok("window.close", json!({"windowId": win})).await;
    assert_eq!(c.ok("pane.list", json!({})).await, json!([]));
    let saved = read_state(&state_path);
    assert_eq!(
        (saved["workspaces"][0]["windows"].clone(), saved["panes"].clone()),
        (json!([]), json!([]))
    );
}

#[test]
fn a_broken_file_stops_the_core_from_starting() {
    let tmp = TempDir::new().unwrap();
    let state_path = tmp.path().join("persistence.json");
    fs::write(&state_path, r#"{"version": 2, "workspaces": [], "panes": []}"#).unwrap();
    let err = Core::new(CoreOptions {
        version: None,
        socket_path: tmp.path().join("s.sock"),
        limits: Limits::default(),
        log: Logger::new(LogLevel::Error, |_| {}),
        state_path: Some(state_path.clone()),
        profiles: builtin_profiles().unwrap(),
    })
    .err()
    .expect("a version 2 file must be refused");
    assert!(err.starts_with("invalid persistence file"), "{err}");
    assert!(err.contains(&state_path.display().to_string()), "{err}");
}

#[tokio::test]
async fn resizes_are_saved_after_a_short_delay_and_when_the_daemon_stops() {
    let tmp = TempDir::new().unwrap();
    let state_path = tmp.path().join("persistence.json");
    let core = core_at(&state_path);
    let mut c = connect(&core);
    let pane_id = c
        .ok(
            "pane.open",
            json!({"cmd": sh("sleep 30"), "cols": 80, "rows": 24}),
        )
        .await["paneId"]
        .as_str()
        .unwrap()
        .to_string();
    c.ok("pane.resize", json!({"paneId": pane_id, "cols": 120, "rows": 50}))
        .await;
    // すぐには書かない（連続した resize を 1 回の保存にまとめる）
    assert_eq!(read_state(&state_path)["panes"][0]["cols"], 80);
    wait_until("the delayed save", || async {
        read_state(&state_path)["panes"][0]["cols"] == 120
    })
    .await;
    assert_eq!(read_state(&state_path)["panes"][0]["rows"], 50);

    // 期限の前に終了しても、遅らせていた保存は済ませる。
    c.ok("pane.resize", json!({"paneId": pane_id, "cols": 70, "rows": 20}))
        .await;
    core.shutdown().await;
    assert_eq!(read_state(&state_path)["panes"][0]["cols"], 70);
}

#[tokio::test]
async fn a_failed_save_undoes_the_change_and_emits_no_event() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("data");
    fs::create_dir(&dir).unwrap();
    let state_path = dir.join("persistence.json");
    let core = core_at(&state_path);
    let mut c = connect(&core);
    let mut watcher = connect(&core);
    watcher.ok("events.subscribe", json!({})).await;
    let head = c.ok("server.info", json!({})).await["eventHead"]
        .as_u64()
        .unwrap();

    c.ok("workspace.create", json!({"name": "kept"})).await;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    if fs::File::create(dir.join("probe")).is_ok() {
        return; // root は書き込めてしまうので、この確認はできない
    }
    assert_eq!(
        c.err_code("workspace.create", json!({"name": "lost"})).await,
        INTERNAL
    );
    assert_eq!(
        c.err_code("pane.open", json!({"cmd": sh("sleep 30")})).await,
        INTERNAL
    );
    let names: Vec<Value> = c
        .ok("workspace.list", json!({}))
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].clone())
        .collect();
    assert_eq!(
        names,
        vec![json!("kept")],
        "the failed creations were undone, including the default workspace"
    );
    assert_eq!(c.ok("pane.list", json!({})).await, json!([]));

    // 失敗した変更の分のイベントは出ない（出たのは kept の作成だけ）。
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = c.ok("server.info", json!({})).await["eventHead"]
        .as_u64()
        .unwrap();
    assert_eq!(events, head + 1);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    c.ok("workspace.create", json!({"name": "again"})).await;
}
