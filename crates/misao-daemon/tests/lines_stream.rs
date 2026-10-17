//! 行ストリーム（pane.subscribe_lines）: ANSI の除去、since / epoch / gap、置き換え、追い越された購読者の切断。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use misao_daemon::Limits;
use serde_json::{Value, json};

use common::{Client, connect, connect_with_buffer, core, core_with, sh};

async fn open(c: &mut Client, script: &str) -> String {
    let r = c
        .ok("pane.open", json!({"cmd": sh(script), "cols": 100, "rows": 30}))
        .await;
    r["paneId"].as_str().unwrap().to_string()
}

async fn wait_exit(c: &mut Client, pane_id: &str) {
    let deadline = std::time::Instant::now() + common::WAIT;
    loop {
        if c.ok("pane.info", json!({"paneId": pane_id})).await["processState"] == "exited" {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "the pane did not exit");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 最初の出力（行リングへの記録を含む）が済むまで待つ。購読はしない（古い購読が先頭の行を配信してしまうため）。
async fn wait_first_line(c: &mut Client, pane_id: &str) {
    let deadline = std::time::Instant::now() + common::WAIT;
    while c.ok("pane.info", json!({"paneId": pane_id})).await["lastOutputAt"].is_null() {
        assert!(std::time::Instant::now() < deadline, "no output");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn texts(notifications: &[Value]) -> Vec<String> {
    notifications
        .iter()
        .map(|n| n["params"]["text"].as_str().unwrap().to_string())
        .collect()
}

fn seqs(notifications: &[Value]) -> Vec<u64> {
    notifications
        .iter()
        .map(|n| n["params"]["seq"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn lines_have_ansi_removed_and_a_final_unterminated_line_is_flushed_on_exit() {
    let mut c = connect(&core());
    let epoch = c.ok("server.info", json!({})).await["epoch"].clone();
    let pane_id = open(
        &mut c,
        r"printf '\033[31mred\033[0m text\r\n\033]0;title\007second\nthird'",
    )
    .await;
    let r = c
        .ok(
            "pane.subscribe_lines",
            json!({"paneId": pane_id, "since": 0, "epoch": epoch}),
        )
        .await;
    assert_eq!(r["gap"], false);
    assert_eq!(r["epoch"], epoch);
    let n = c.notifications(3).await;
    assert_eq!(texts(&n), ["red text", "second", "third"]);
    assert_eq!(seqs(&n), [1, 2, 3]);
    assert_eq!(n[0]["method"], "pane.line");
    assert_eq!(n[0]["params"]["paneId"], pane_id);
    assert!(n[0]["params"]["ts"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn without_since_only_new_lines_are_delivered() {
    let mut c = connect(&core());
    let pane_id = open(&mut c, "stty -echo; echo ready; read x; echo after").await;
    wait_first_line(&mut c, &pane_id).await;
    let r = c.ok("pane.subscribe_lines", json!({"paneId": pane_id})).await;
    assert_eq!((r["gap"].clone(), r["head"].clone()), (json!(false), json!(1)));
    c.ok("pane.write", json!({"paneId": pane_id, "data": "go\n"}))
        .await;
    let n = c.notifications(1).await;
    assert_eq!(texts(&n), ["after"]);
    assert_eq!(seqs(&n), [2], "ready (seq 1) was before the subscription");
}

#[tokio::test]
async fn since_epoch_and_gap_follow_the_ring() {
    // 1 行 = 文字数 + 64。"line N" は 6 文字で 70 なので、200 バイトのリングは直近 2 行を保つ。
    let limits = Limits {
        lines_ring_bytes: 200,
        ..Limits::default()
    };
    let mut c = connect(&core_with(limits));
    let epoch = c.ok("server.info", json!({})).await["epoch"].clone();
    let pane_id = open(&mut c, "for i in 1 2 3 4 5; do echo line $i; done").await;
    wait_exit(&mut c, &pane_id).await;
    let sub = |since: Value| json!({"paneId": pane_id, "since": since, "epoch": epoch});

    // 保持範囲は 4..=5。since=3 は連続している（gap なし）。
    let r = c.ok("pane.subscribe_lines", sub(json!(3))).await;
    assert_eq!((r["gap"].clone(), r["head"].clone()), (json!(false), json!(5)));
    assert_eq!(texts(&c.notifications(2).await), ["line 4", "line 5"]);

    // 追い出された範囲を含む since は gap で、保持している最古から再生する。
    let r = c.ok("pane.subscribe_lines", sub(json!(1))).await;
    assert_eq!(r["gap"], true);
    assert_eq!(texts(&c.notifications(2).await), ["line 4", "line 5"]);

    // head ちょうどは gap なし・再生なし。
    let r = c.ok("pane.subscribe_lines", sub(json!(5))).await;
    assert_eq!(r["gap"], false);
    c.assert_no_notification(Duration::from_millis(200)).await;

    // head より先（送信側の seq が巻き戻った）は gap で、再生しない。
    let r = c.ok("pane.subscribe_lines", sub(json!(1000))).await;
    assert_eq!(r["gap"], true);
    c.assert_no_notification(Duration::from_millis(200)).await;

    // 別の epoch の since は gap で、保持している最古から再生する。
    let other = "0".repeat(26);
    let r = c
        .ok(
            "pane.subscribe_lines",
            json!({"paneId": pane_id, "since": 5, "epoch": other}),
        )
        .await;
    assert_eq!((r["gap"].clone(), r["epoch"].clone()), (json!(true), epoch));
    assert_eq!(texts(&c.notifications(2).await), ["line 4", "line 5"]);
}

#[tokio::test]
async fn resubscribing_replaces_the_previous_subscription() {
    let mut c = connect(&core());
    let pane_id = open(&mut c, "stty -echo; echo ready; read x; echo after").await;
    wait_first_line(&mut c, &pane_id).await;
    c.ok("pane.subscribe_lines", json!({"paneId": pane_id})).await;
    c.ok("pane.write", json!({"paneId": pane_id, "data": "go\n"}))
        .await;
    assert_eq!(texts(&c.notifications(1).await), ["after"]);
    c.assert_no_notification(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn a_subscriber_that_never_reads_is_cut_off_once_the_ring_overtakes_it() {
    let limits = Limits {
        lines_ring_bytes: 8 * 1024,
        ..Limits::default()
    };
    let core = core_with(limits);
    let mut control = connect(&core);
    let mut slow = connect_with_buffer(&core, 1024);
    let pane_id = open(
        &mut control,
        "yes 0123456789abcdef0123456789abcdef | head -n 200000",
    )
    .await;
    slow.ok(
        "pane.subscribe_lines",
        json!({"paneId": pane_id, "since": 0, "epoch": control.ok("server.info", json!({})).await["epoch"]}),
    )
    .await;
    wait_exit(&mut control, &pane_id).await;
    // 読み始めると、バッファに残っていた分の後で接続が閉じられている。
    slow.drain_until_closed().await;
    // 他の接続は影響を受けず、再購読は gap で知らされる。
    let epoch = control.ok("server.info", json!({})).await["epoch"].clone();
    let r = control
        .ok(
            "pane.subscribe_lines",
            json!({"paneId": pane_id, "since": 1, "epoch": epoch}),
        )
        .await;
    assert_eq!(r["gap"], true);
}

#[tokio::test]
async fn closing_the_pane_stops_its_subscriptions_without_breaking_the_connection() {
    let mut c = connect(&core());
    let pane_id = open(&mut c, "echo hi; sleep 300").await;
    c.ok("pane.subscribe_lines", json!({"paneId": pane_id, "since": 0}))
        .await;
    assert_eq!(texts(&c.notifications(1).await), ["hi"]);
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    assert_eq!(c.ok("server.info", json!({})).await["paneCount"], 0);
    assert_eq!(
        c.err_code("pane.subscribe_lines", json!({"paneId": pane_id}))
            .await,
        1001
    );
}

/// 読んでいる（カーソルが進んでいる）が子より遅い購読者は、リングが小さくても取りこぼさない。
/// 子の出力を止めて待つので、リングに追い越されて切断されることがない。
#[tokio::test]
async fn a_slow_but_reading_subscriber_never_loses_lines() {
    const LINES: u64 = 60_000;
    let limits = Limits {
        lines_ring_bytes: 256 * 1024,
        ..Limits::default()
    };
    let core = core_with(limits);
    let mut control = connect(&core);
    let mut slow = connect_with_buffer(&core, 4 * 1024);
    let epoch = control.ok("server.info", json!({})).await["epoch"].clone();
    let pane_id = open(
        &mut control,
        &format!("yes 0123456789abcdef0123456789abcdef | head -n {LINES}"),
    )
    .await;
    let r = slow
        .ok(
            "pane.subscribe_lines",
            json!({"paneId": pane_id, "since": 0, "epoch": epoch}),
        )
        .await;
    assert_eq!(r["gap"], false);
    let (mut received, mut since_pause) = (0u64, 0u32);
    while received < LINES {
        let n = slow.next_notification().await;
        received += 1;
        assert_eq!(n["params"]["seq"], received, "no gap, no duplicate, in order");
        since_pause += 1;
        if since_pause == 200 {
            since_pause = 0;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    wait_exit(&mut control, &pane_id).await;
}
