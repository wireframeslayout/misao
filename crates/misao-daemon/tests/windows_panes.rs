//! Windows の ConPTY のペイン: Job Object によるプロセスツリーの終了、PATHEXT と `.ps1` の解決、起動失敗のメッセージ、
//! resize の再描画が行ストリームに入らないこと。
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use common::{Client, connect, core, is_alive, read_file_when_ready, wait_until};

const INVALID_PARAMS: i64 = -32602;

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
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let i = info(c, pane_id).await;
        if i["processState"] == "exited" {
            return i;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the pane to exit"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// PATH の先頭に `dir` を足した値（子の環境に渡す）。
fn path_with(dir: &std::path::Path) -> String {
    format!("{};{}", dir.display(), std::env::var("PATH").unwrap_or_default())
}

/// 行ストリームに届いた行を先頭から全部集める（新しい接続で購読する）。
async fn lines_of(core: &std::sync::Arc<misao_daemon::Core>, pane_id: &str) -> Vec<String> {
    let mut c = connect(core);
    let epoch = c.ok("server.info", json!({})).await["epoch"].clone();
    let reply = c
        .ok(
            "pane.subscribe_lines",
            json!({"paneId": pane_id, "since": 0, "epoch": epoch}),
        )
        .await;
    let head = usize::try_from(reply["head"].as_u64().unwrap()).unwrap();
    let notifications = c.notifications(head).await;
    notifications
        .iter()
        .map(|n| n["params"]["text"].as_str().unwrap().to_string())
        .collect()
}

async fn screen_text(c: &mut Client, pane_id: &str) -> String {
    c.ok("pane.screen", json!({"paneId": pane_id})).await["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn closing_a_pane_kills_grandchildren_through_the_job_object() {
    let tmp = TempDir::new().unwrap();
    let pid_file = tmp.path().join("grandchild.pid");
    let mut c = connect(&core());
    // powershell が ping を子として起動する（ping は孫プロセス）
    let script = format!(
        "$p = Start-Process ping -ArgumentList '-n','300','127.0.0.1' -PassThru -WindowStyle Hidden; \
         Set-Content -Path '{}' -Value $p.Id; Start-Sleep 300",
        pid_file.display()
    );
    let pane = open(
        &mut c,
        json!(["powershell.exe", "-NoProfile", "-Command", script]),
        json!({}),
    )
    .await;
    let grandchild = {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && !text.trim().is_empty()
            {
                break text.trim().to_string();
            }
            assert!(Instant::now() < deadline, "the grandchild did not start");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    assert!(is_alive(&grandchild));
    let started = Instant::now();
    c.ok("pane.close", json!({"paneId": pane})).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "close took {:?}",
        started.elapsed()
    );
    wait_until("the grandchild to be killed", || async { !is_alive(&grandchild) }).await;
}

#[tokio::test]
async fn names_without_an_extension_are_resolved_through_pathext() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out.txt");
    std::fs::write(
        tmp.path().join("hello.cmd"),
        format!("@echo off\r\necho hi %1> \"{}\"\r\nexit /b 3\r\n", out.display()),
    )
    .unwrap();
    let mut c = connect(&core());
    let pane = open(
        &mut c,
        json!(["hello", "world"]),
        json!({"env": {"PATH": path_with(tmp.path())}}),
    )
    .await;
    assert_eq!(read_file_when_ready(&out).await.trim(), "hi world");
    let done = wait_exit(&mut c, &pane).await;
    assert_eq!(done["exitCode"], 3);
}

#[tokio::test]
async fn a_ps1_script_is_run_through_powershell() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out.txt");
    std::fs::write(
        tmp.path().join("task.ps1"),
        format!(
            "param($Name)\r\nSet-Content -Path '{}' -Value \"ps1:$Name\"\r\nexit 5\r\n",
            out.display()
        ),
    )
    .unwrap();
    let mut c = connect(&core());
    let pane = open(
        &mut c,
        json!(["task.ps1", "-Name", "x"]),
        json!({"env": {"PATH": path_with(tmp.path())}}),
    )
    .await;
    assert_eq!(read_file_when_ready(&out).await.trim(), "ps1:x");
    assert_eq!(wait_exit(&mut c, &pane).await["exitCode"], 5);
}

#[tokio::test]
async fn a_missing_command_is_reported_with_its_name_and_leaves_no_pane() {
    let mut c = connect(&core());
    let response = c.call("pane.open", json!({"cmd": ["no-such-command-xyz"]})).await;
    assert_eq!(response["error"]["code"], INVALID_PARAMS);
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("no-such-command-xyz"), "{message}");
    assert_eq!(c.ok("pane.list", json!({})).await.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn the_exit_code_of_a_normal_command_is_reported() {
    let mut c = connect(&core());
    let pane = open(&mut c, json!(["cmd.exe", "/c", "exit 7"]), json!({})).await;
    assert_eq!(wait_exit(&mut c, &pane).await["exitCode"], 7);
}

#[tokio::test]
async fn resize_repaints_do_not_duplicate_lines_or_add_blank_ones() {
    let core = core();
    let mut c = connect(&core);
    let pane = open(&mut c, json!(["cmd.exe", "/q"]), json!({"cols": 80, "rows": 24})).await;
    c.ok("pane.write", json!({"paneId": pane, "data": "echo marker-one\r"}))
        .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !screen_text(&mut c, &pane).await.contains("marker-one") {
        assert!(Instant::now() < deadline, "the output did not appear");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = lines_of(&core, &pane).await;
    assert!(before.iter().any(|l| l.contains("marker-one")), "{before:?}");

    // 大きさを変えるたびに ConPTY が画面全体を書き直す。その再描画は行ストリームに入らない。
    for (cols, rows) in [(100, 30), (80, 24), (132, 50), (80, 24)] {
        c.ok("pane.resize", json!({"paneId": pane, "cols": cols, "rows": rows}))
            .await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    let after = lines_of(&core, &pane).await;
    assert_eq!(after, before, "the repaints leaked into the line stream");
    c.ok("pane.close", json!({"paneId": pane})).await;
}

/// 子が `ESC[6n` を出すと、デーモンが 1 回だけ返事をし、問い合わせは attach への配信（raw の再生を含む）に載らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cursor_query_is_answered_once_and_kept_out_of_the_attach_stream() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("reply.json");
    let script = format!(
        "const fs = require('fs'); let got = ''; process.stdin.setRawMode(true); process.stdin.on('data', (d) => {{ got += String(d); }});
         process.stdout.write('A\\x1b[6nB'); setTimeout(() => {{ fs.writeFileSync({out:?}, JSON.stringify(got)); setTimeout(() => process.exit(0), 3000); }}, 1500);",
        out = out.display().to_string()
    );
    let core = core();
    let mut c = connect(&core);
    let node = std::env::var("MISAO_TEST_NODE").unwrap_or_else(|_| "node".to_string());
    let pane = open(&mut c, json!([node, "-e", script]), json!({})).await;
    // 出力の前から attach している接続 (live の配信) と、出力の後に raw で再生する接続 (リング) の両方で確かめる
    let mut live = connect(&core);
    live.ok(
        "pane.attach",
        json!({"paneId": pane, "clientId": "live", "replay": "none"}),
    )
    .await;

    wait_until("the child to write its reply", || async { out.exists() }).await;
    let reply: String = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert!(
        reply.starts_with("\u{1b}[") && reply.ends_with('R') && reply.matches('\u{1b}').count() == 1,
        "exactly one cursor report: {reply:?}"
    );

    let live_bytes = live.collect_output(Duration::from_millis(800)).await;
    let mut late = connect(&core);
    late.ok(
        "pane.attach",
        json!({"paneId": pane, "clientId": "late", "replay": "raw"}),
    )
    .await;
    let late_bytes = late.collect_output(Duration::from_millis(800)).await;
    for (name, bytes) in [("live", live_bytes), ("replay", late_bytes)] {
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("\u{1b}[6n"),
            "{name}: the query reached the client: {text:?}"
        );
    }
}

/// リーダーが先に終わり、孫が 200 ms より後に出力しても、その出力は行ストリームに入ってから `pane.exited` になる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_of_a_grandchild_after_the_leader_exits_is_not_lost() {
    let core = core();
    let mut c = connect(&core);
    // powershell はすぐ終わり、孫の cmd が約 2 秒後に late-output を出す (同じコンソールに)。
    let script = "Start-Process cmd.exe -ArgumentList '/c','ping -n 3 127.0.0.1 >nul & echo late-output' -NoNewWindow";
    let pane = open(&mut c, json!(["powershell.exe", "-NoProfile", "-Command", script]), json!({})).await;
    let done = wait_exit(&mut c, &pane).await;
    assert_eq!(done["exitCode"], 0);
    let lines = lines_of(&core, &pane).await;
    assert!(lines.iter().any(|l| l.contains("late-output")), "{lines:?}");
    // 孫が終わって、Job は空になっている
    c.ok("pane.close", json!({"paneId": pane})).await;
}
