//! pane.screen / タイトル / 稼働判定 / 前面プロセス（実際に `sh` を起動して確かめる）。
//! 内容の細かい一致は Node 版との差分テスト（`src/node_diff_tests.rs`）が確かめる。ここは配線を確かめる。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use common::{Client, WAIT, connect, core, sh};

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

/// 画面が条件を満たすまで待って、その画面を返す。
async fn wait_screen(c: &mut Client, pane_id: &str, what: &str, cond: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let screen = c.ok("pane.screen", json!({"paneId": pane_id})).await;
        if cond(&screen) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {screen}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 次のイベントのうち、種類が `kind` のもの（他は読み飛ばす）。
async fn next_event(c: &mut Client, kind: &str) -> Value {
    loop {
        let event = c.next_notification().await;
        if event["params"]["type"] == kind {
            return event["params"].clone();
        }
    }
}

#[tokio::test]
async fn pane_screen_shows_the_viewport_cursor_and_title_and_counts_activity() {
    let mut c = connect(&core());
    let script = "printf '\\033]0;hello title\\007one\\r\\ntwo'; sleep 30";
    let pane = open(&mut c, sh(script), json!({"cols": 20, "rows": 4})).await;
    let screen = wait_screen(&mut c, &pane, "the output", |s| {
        s["text"].as_str().is_some_and(|t| t.contains("two"))
    })
    .await;
    assert_eq!(screen["text"], "one\ntwo\n\n");
    assert_eq!(screen["cursor"], json!({"x": 3, "y": 1}));
    assert_eq!(
        (&screen["altScreen"], &screen["title"]),
        (&json!(false), &json!("hello title"))
    );
    let before = screen["activity"].as_u64().unwrap();
    assert!(before >= 1);
    c.ok("pane.resize", json!({"paneId": pane, "cols": 30, "rows": 5}))
        .await;
    let after = c.ok("pane.screen", json!({"paneId": pane})).await;
    assert_eq!(after["activity"].as_u64().unwrap(), before + 1);
    assert_eq!(after["text"].as_str().unwrap().split('\n').count(), 5);
    assert_eq!(
        c.ok("pane.info", json!({"paneId": pane})).await["title"],
        "hello title"
    );
}

#[tokio::test]
async fn an_exited_pane_keeps_answering_pane_screen() {
    let mut c = connect(&core());
    let pane = open(&mut c, sh("printf 'bye'"), json!({"cols": 10, "rows": 2})).await;
    let screen = wait_screen(&mut c, &pane, "the output", |s| {
        s["text"].as_str().is_some_and(|t| t.contains("bye"))
    })
    .await;
    assert_eq!(screen["text"], "bye\n");
    let unknown = format!("p_{}", "0".repeat(26));
    assert_eq!(c.err_code("pane.screen", json!({"paneId": unknown})).await, 1001);
}

#[tokio::test]
async fn title_and_state_events_follow_the_screen_in_order() {
    let mut c = connect(&core());
    c.ok("events.subscribe", json!({})).await;
    // タイトルのスピナーが付くと title 段で working、通常のタイトルになると idle、終了で exit 段。
    let script = "printf '\\033]0;⠋ build\\007'; sleep 0.3; printf '\\033]0;done\\007'; sleep 0.3";
    let pane = open(&mut c, sh(script), json!({})).await;
    let spinner = next_event(&mut c, "pane.state").await;
    assert_eq!(
        (
            &spinner["paneId"],
            &spinner["data"]["state"],
            &spinner["data"]["decidedBy"],
            &spinner["data"]["prev"]
        ),
        (
            &json!(pane),
            &json!("working"),
            &json!("title"),
            &json!("unknown")
        )
    );
    let title = next_event(&mut c, "pane.title").await;
    assert_eq!(title["data"]["title"], "⠋ build");
    let done = next_event(&mut c, "pane.state").await;
    assert_eq!(
        (&done["data"]["state"], &done["data"]["decidedBy"]),
        (&json!("idle"), &json!("title"))
    );
    assert_eq!(next_event(&mut c, "pane.title").await["data"]["title"], "done");
    assert_eq!(next_event(&mut c, "pane.exited").await["paneId"], json!(pane));
    let exited = next_event(&mut c, "pane.state").await;
    assert_eq!(
        (
            &exited["data"]["state"],
            &exited["data"]["decidedBy"],
            &exited["data"]["prev"]
        ),
        (&json!("exited"), &json!("exit"), &json!("idle"))
    );
}

/// 起動コマンドの名前が `claude` なら、Claude Code の画面規則（プロファイル）で判定する。
#[tokio::test]
async fn a_pane_running_claude_is_judged_by_the_claude_profile() {
    let tmp = TempDir::new().unwrap();
    let fake = tmp.path().join("claude");
    let rule = "─".repeat(40);
    let script = format!(
        "#!/bin/sh\nprintf 'Do you want to proceed?\\r\\n %s\\r\\n' '{rule}'\nprintf ' ❯ 1. Yes\\r\\n   2. No\\r\\n'\nsleep 30\n"
    );
    std::fs::write(&fake, script).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut c = connect(&core());
    let pane = open(&mut c, json!([fake]), json!({"cols": 60, "rows": 10})).await;
    let deadline = Instant::now() + WAIT;
    loop {
        let info = c.ok("pane.info", json!({"paneId": pane})).await;
        if info["agentState"] == "blocked" {
            assert_eq!(info["decidedBy"], "claude");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the profile never judged the pane: {info}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn pane_info_reports_the_foreground_command_while_running() {
    let mut c = connect(&core());
    let pane = open(&mut c, sh("exec sleep 30"), json!({})).await;
    let deadline = Instant::now() + WAIT;
    loop {
        let info = c.ok("pane.info", json!({"paneId": pane})).await;
        if info["fgCommand"] == "sleep" {
            break;
        }
        assert!(Instant::now() < deadline, "no fgCommand: {info}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    c.ok("pane.close", json!({"paneId": pane})).await;
}

#[tokio::test]
async fn an_exited_pane_has_no_foreground_command() {
    let mut c = connect(&core());
    let pane = open(&mut c, sh("exit 0"), json!({})).await;
    let deadline = Instant::now() + WAIT;
    loop {
        let info = c.ok("pane.info", json!({"paneId": pane})).await;
        if info["processState"] == "exited" {
            assert!(info.get("fgCommand").is_none(), "{info}");
            break;
        }
        assert!(Instant::now() < deadline, "the pane did not exit");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
