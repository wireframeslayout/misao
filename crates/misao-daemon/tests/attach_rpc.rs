//! pane.attach / pane.detach: 再生（raw / snapshot / none）、複数クライアント、サイズの調停、切断の後始末。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use common::{Client, connect, core, sh};

const PANE_NOT_FOUND: i64 = 1001;
const UNSUPPORTED: i64 = 1004;
const INVALID_PARAMS: i64 = -32602;

async fn open(c: &mut Client, script: &str) -> String {
    c.ok("pane.open", json!({"cmd": sh(script), "cols": 40, "rows": 10}))
        .await["paneId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn text_of(n: &Value) -> String {
    String::from_utf8(STANDARD.decode(n["params"]["dataB64"].as_str().unwrap()).unwrap()).unwrap()
}

/// `pane.output` を、出力に `needle` が現れるまで集める。
async fn collect_until(c: &mut Client, needle: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let n = c.next_notification().await;
        if n["method"] != "pane.output" {
            continue;
        }
        seen.push(n);
        if seen.iter().map(text_of).collect::<String>().contains(needle) {
            return seen;
        }
    }
}

/// pane.screen に `needle` が現れるまで待つ。
async fn wait_screen(c: &mut Client, pane_id: &str, needle: &str) {
    for _ in 0..500 {
        if c.ok("pane.screen", json!({"paneId": pane_id})).await["text"]
            .as_str()
            .unwrap()
            .contains(needle)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {needle:?}");
}

async fn info(c: &mut Client, pane_id: &str) -> Value {
    c.ok("pane.info", json!({"paneId": pane_id})).await
}

#[tokio::test]
async fn raw_replay_sends_the_ring_then_live_output_without_gaps_or_duplicates() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(&mut owner, "echo first; read x; echo second:$x; sleep 30").await;
    wait_screen(&mut owner, &pane_id, "first").await;

    let mut view = connect(&core);
    let result = view
        .ok(
            "pane.attach",
            json!({"paneId": pane_id, "clientId": "A", "replay": "raw"}),
        )
        .await;
    assert_eq!(result["truncated"], false);
    assert!(
        result["head"].as_u64().unwrap() >= 1 && result["oldest"] == 1,
        "{result}"
    );
    owner
        .ok("pane.write", json!({"paneId": pane_id, "data": "go\n"}))
        .await;
    let seen = collect_until(&mut view, "second:go").await;
    let seqs: Vec<u64> = seen
        .iter()
        .map(|n| n["params"]["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs[0], 1, "the replay starts from the oldest output");
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "contiguous: {seqs:?}");
    let all: String = seen.iter().map(text_of).collect();
    assert!(all.contains("first") && all.contains("second:go"), "{all:?}");
    assert!(
        seen.iter()
            .all(|n| n["params"]["paneId"] == json!(pane_id) && n["params"].get("replay").is_none())
    );
}

#[tokio::test]
async fn snapshot_replay_is_one_notification_marked_replay_snapshot_then_live_output() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(
        &mut owner,
        "printf '\\033[31mred\\033[0m line\\n'; read x; echo live:$x; sleep 30",
    )
    .await;
    wait_screen(&mut owner, &pane_id, "red line").await;
    let mut view = connect(&core);
    let result = view
        .ok(
            "pane.attach",
            json!({"paneId": pane_id, "clientId": "A", "replay": "snapshot"}),
        )
        .await;
    assert_eq!(result["truncated"], false);
    let head = result["head"].as_u64().unwrap();
    let first = view.next_notification().await;
    assert_eq!(first["params"]["replay"], "snapshot");
    assert_eq!(first["params"]["seq"].as_u64(), Some(head));
    let snapshot = text_of(&first);
    assert!(snapshot.starts_with("\x1b[0m\x1b[2J\x1b[H"), "{snapshot:?}");
    assert!(
        snapshot.contains("\x1b[0;31mred\x1b[0m line"),
        "colors are replayed: {snapshot:?}"
    );
    owner
        .ok("pane.write", json!({"paneId": pane_id, "data": "ok\n"}))
        .await;
    let live = collect_until(&mut view, "live:ok").await;
    assert!(
        live.iter().all(|n| n["params"]["seq"].as_u64().unwrap() > head),
        "no overlap with the snapshot"
    );
}

#[tokio::test]
async fn no_replay_sends_only_new_output() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(&mut owner, "echo old; read x; echo new:$x; sleep 30").await;
    wait_screen(&mut owner, &pane_id, "old").await;
    let mut view = connect(&core);
    view.ok("pane.attach", json!({"paneId": pane_id, "clientId": "A"}))
        .await;
    owner
        .ok("pane.write", json!({"paneId": pane_id, "data": "x\n"}))
        .await;
    let seen = collect_until(&mut view, "new:x").await;
    assert!(!seen.iter().map(text_of).collect::<String>().contains("old"));
}

#[tokio::test]
async fn the_pane_size_follows_the_last_client_to_act_and_returns_when_the_owner_leaves() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(&mut owner, "sleep 30").await;
    let mut events = connect(&core);
    events.ok("events.subscribe", json!({})).await;
    let (mut a, mut b) = (connect(&core), connect(&core));
    a.ok(
        "pane.attach",
        json!({"paneId": pane_id, "clientId": "A", "cols": 120, "rows": 40}),
    )
    .await;
    let i = info(&mut owner, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["rows"].as_u64(), i["sizeOwner"].as_str()),
        (Some(120), Some(40), Some("A"))
    );
    b.ok(
        "pane.attach",
        json!({"paneId": pane_id, "clientId": "B", "cols": 80, "rows": 24}),
    )
    .await;
    let i = info(&mut owner, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["sizeOwner"].as_str()),
        (Some(80), Some("B"))
    );
    assert_eq!(i["clients"], json!(["A", "B"]));

    // A が入力すると、A のサイズに戻る
    a.ok(
        "pane.write",
        json!({"paneId": pane_id, "data": "k", "clientId": "A", "source": "terminal"}),
    )
    .await;
    let i = info(&mut owner, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["rows"].as_u64(), i["sizeOwner"].as_str()),
        (Some(120), Some(40), Some("A"))
    );
    b.ok(
        "pane.write",
        json!({"paneId": pane_id, "data": "k", "clientId": "B", "source": "terminal"}),
    )
    .await;
    assert_eq!(info(&mut owner, &pane_id).await["sizeOwner"], "B");

    // 所有者が detach すると、残ったクライアントのサイズに戻る
    b.ok("pane.detach", json!({"paneId": pane_id})).await;
    let i = info(&mut owner, &pane_id).await;
    assert_eq!(
        (i["cols"].as_u64(), i["sizeOwner"].as_str(), i["clients"].clone()),
        (Some(120), Some("A"), json!(["A"]))
    );

    // 接続が切れても同じ後始末
    drop(a);
    for _ in 0..500 {
        if info(&mut owner, &pane_id).await["clients"] == json!([]) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(info(&mut owner, &pane_id).await["clients"], json!([]));
    let mut kinds = Vec::new();
    while kinds
        .iter()
        .filter(|k: &&String| k.as_str() == "client.detached")
        .count()
        < 2
    {
        let n = events.next_notification().await;
        kinds.push(n["params"]["type"].as_str().unwrap().to_string());
    }
    assert_eq!(
        kinds.iter().filter(|k| k.as_str() == "client.attached").count(),
        2
    );
    assert!(kinds.contains(&"pane.resized".to_string()));
}

#[tokio::test]
async fn attaching_again_with_the_same_client_id_replaces_the_destination_without_detaching() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(&mut owner, "read x; echo got:$x; sleep 30").await;
    let mut events = connect(&core);
    events.ok("events.subscribe", json!({})).await;
    let mut view = connect(&core);
    view.ok("pane.attach", json!({"paneId": pane_id, "clientId": "A"}))
        .await;
    view.ok("pane.attach", json!({"paneId": pane_id, "clientId": "A"}))
        .await;
    owner
        .ok("pane.write", json!({"paneId": pane_id, "data": "z\n"}))
        .await;
    let seen = collect_until(&mut view, "got:z").await;
    let seqs: Vec<u64> = seen
        .iter()
        .map(|n| n["params"]["seq"].as_u64().unwrap())
        .collect();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "each output arrives once: {seqs:?}"
    );
    assert_eq!(info(&mut owner, &pane_id).await["clients"], json!(["A"]));
    view.ok("pane.detach", json!({"paneId": pane_id})).await;
    let mut kinds = Vec::new();
    while !kinds.contains(&"client.detached".to_string()) {
        kinds.push(
            events.next_notification().await["params"]["type"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(
        kinds.iter().filter(|k| k.as_str() == "client.attached").count(),
        1
    );
}

#[tokio::test]
async fn closing_a_pane_detaches_its_clients_before_pane_closed() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(&mut owner, "sleep 30").await;
    let mut events = connect(&core);
    events.ok("events.subscribe", json!({"since": 0})).await;
    let mut view = connect(&core);
    view.ok("pane.attach", json!({"paneId": pane_id, "clientId": "A"}))
        .await;
    owner.ok("pane.close", json!({"paneId": pane_id})).await;
    let mut kinds = Vec::new();
    while !kinds.contains(&"pane.closed".to_string()) {
        kinds.push(
            events.next_notification().await["params"]["type"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let detached = kinds.iter().position(|k| k == "client.detached").unwrap();
    assert!(
        detached < kinds.iter().position(|k| k == "pane.closed").unwrap(),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn attach_rejects_cells_unknown_panes_and_bad_params() {
    let core = core();
    let mut c = connect(&core);
    let pane_id = open(&mut c, "sleep 30").await;
    assert_eq!(
        c.err_code(
            "pane.attach",
            json!({"paneId": pane_id, "clientId": "A", "mode": "cells"})
        )
        .await,
        UNSUPPORTED
    );
    let missing = format!("p_{}", "0".repeat(26));
    assert_eq!(
        c.err_code("pane.attach", json!({"paneId": missing, "clientId": "A"}))
            .await,
        PANE_NOT_FOUND
    );
    for bad in [
        json!({"paneId": pane_id}),
        json!({"paneId": pane_id, "clientId": ""}),
        json!({"paneId": pane_id, "clientId": "A", "replay": "all"}),
    ] {
        assert_eq!(c.err_code("pane.attach", bad).await, INVALID_PARAMS);
    }
    // detach は attach していなくても ok
    assert_eq!(
        c.ok("pane.detach", json!({"paneId": pane_id})).await,
        json!({"ok": true})
    );
    assert!(
        info(&mut c, &pane_id).await["clients"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_detach_and_an_attach_of_the_same_client_id_from_other_connections_keep_the_client() {
    let core = core();
    let mut owner = connect(&core);
    let pane_id = open(&mut owner, "sleep 30").await;
    for round in 0..40 {
        let (mut a, mut b) = (connect(&core), connect(&core));
        a.ok(
            "pane.attach",
            json!({"paneId": pane_id, "clientId": "C", "cols": 100, "rows": 30}),
        )
        .await;
        let detach = a.ok("pane.detach", json!({"paneId": pane_id}));
        let attach = b.ok(
            "pane.attach",
            json!({"paneId": pane_id, "clientId": "C", "cols": 90, "rows": 20}),
        );
        tokio::join!(detach, attach);
        let i = info(&mut owner, &pane_id).await;
        assert_eq!(i["clients"], json!(["C"]), "round {round}: b is still attached");
        assert_eq!(i["sizeOwner"], "C", "round {round}");
        drop((a, b));
        for _ in 0..500 {
            if info(&mut owner, &pane_id).await["clients"] == json!([]) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

#[tokio::test]
async fn an_attach_racing_a_close_is_either_refused_or_detached_before_pane_closed() {
    for round in 0..20 {
        let core = core();
        let mut owner = connect(&core);
        let pane_id = open(&mut owner, "sleep 30").await;
        let mut events = connect(&core);
        events.ok("events.subscribe", json!({"since": 0})).await;
        let mut view = connect(&core);
        let attach = view.call("pane.attach", json!({"paneId": pane_id, "clientId": "A"}));
        let close = owner.ok("pane.close", json!({"paneId": pane_id}));
        let (attached, _) = tokio::join!(attach, close);
        let attached_ok = attached.get("error").is_none();
        if !attached_ok {
            let code = attached["error"]["code"].as_i64().unwrap();
            assert!(code == 1001 || code == 1002, "round {round}: {attached}");
        }
        let mut kinds = Vec::new();
        while !kinds.contains(&"pane.closed".to_string()) {
            kinds.push(
                events.next_notification().await["params"]["type"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        let attached_events = kinds.iter().filter(|k| *k == "client.attached").count();
        let detached_events = kinds.iter().filter(|k| *k == "client.detached").count();
        assert_eq!(
            attached_events, detached_events,
            "round {round}: every attach is released before pane.closed: {kinds:?}"
        );
    }
}

#[tokio::test]
async fn an_attach_with_a_size_during_a_close_fails_without_changing_size_events_or_saving() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_path = tmp.path().join("persistence.json");
    let core = common::core_at(&state_path);
    let mut owner = connect(&core);
    // SIGHUP を無視する子: close は猶予（3 秒）の間「始まっている」状態が続く。
    // trap が効く前に close が始まると子が SIGHUP で死ぬので（起動の遅い macOS で起きる）、準備ができてから閉じ始める。
    let ready = tmp.path().join("ready");
    let pane_id = open(&mut owner, &format!("trap '' HUP; touch '{}'; sleep 30", ready.display())).await;
    common::wait_until("the child to ignore SIGHUP", || async { ready.exists() }).await;
    let before = std::fs::read_to_string(&state_path).unwrap();
    let head = owner.ok("server.info", json!({})).await["eventHead"]
        .as_u64()
        .unwrap();
    let mut closer = connect(&core);
    let closing = {
        let id = pane_id.clone();
        tokio::spawn(async move { closer.ok("pane.close", json!({"paneId": id})).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut view = connect(&core);
    let failed = view
        .call(
            "pane.attach",
            json!({"paneId": pane_id, "clientId": "A", "cols": 90, "rows": 30, "replay": "snapshot"}),
        )
        .await;
    assert_eq!(failed["error"]["code"], 1002, "{failed}");
    let i = info(&mut owner, &pane_id).await;
    assert_eq!((i["cols"].as_u64(), i["rows"].as_u64()), (Some(40), Some(10)));
    assert_eq!(
        (i["sizeOwner"].clone(), i["clients"].clone()),
        (Value::Null, json!([]))
    );
    // 保存の予約も無い（1.5 秒待っても、ファイルは変わらない）
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(std::fs::read_to_string(&state_path).unwrap(), before);
    closing.await.unwrap();
    let mut events = connect(&core);
    let replay = events.ok("events.subscribe", json!({"since": head})).await;
    assert_eq!(replay["gap"], false);
    let mut kinds = Vec::new();
    while !kinds.contains(&"pane.closed".to_string()) {
        kinds.push(
            events.next_notification().await["params"]["type"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert!(
        !kinds.contains(&"pane.resized".to_string()) && !kinds.contains(&"client.attached".to_string()),
        "{kinds:?}"
    );
}
