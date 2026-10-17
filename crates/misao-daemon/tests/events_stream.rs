//! events.subscribe: 1 接続 1 ストリーム、seq / epoch / gap による復元、イベントの種類と順序。
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use misao_daemon::Limits;
use serde_json::{Value, json};

use common::{Client, connect, core, core_with, sh};

fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["params"]["type"].as_str().unwrap().to_string())
        .collect()
}

fn seqs(events: &[Value]) -> Vec<u64> {
    events
        .iter()
        .map(|e| e["params"]["seq"].as_u64().unwrap())
        .collect()
}

async fn epoch(c: &mut Client) -> Value {
    c.ok("server.info", json!({})).await["epoch"].clone()
}

#[tokio::test]
async fn the_first_event_is_daemon_started_and_live_events_follow_in_order() {
    let mut c = connect(&core());
    let epoch = epoch(&mut c).await;
    let r = c
        .ok("events.subscribe", json!({"since": 0, "epoch": epoch}))
        .await;
    assert_eq!(
        (r["gap"].clone(), r["head"].clone(), r["epoch"].clone()),
        (json!(false), json!(1), epoch)
    );
    let first = c.next_notification().await;
    assert_eq!(first["method"], "event");
    assert_eq!(first["params"]["type"], "daemon.started");
    assert_eq!(first["params"]["seq"], 1);
    assert_eq!(first["params"]["data"]["pid"], std::process::id());
    assert_eq!(
        first["params"]["data"]["protocolVersion"],
        misao_protocol::PROTOCOL_VERSION
    );
    assert!(first["params"].get("paneId").is_none());

    c.ok("workspace.create", json!({"name": "w"})).await;
    c.ok("workspace.rename", json!({"name": "w", "newName": "x"}))
        .await;
    let win = c
        .ok("window.create", json!({"workspace": "x", "name": "main"}))
        .await;
    c.ok(
        "window.rename",
        json!({"windowId": win["windowId"], "name": "m2"}),
    )
    .await;
    c.ok("window.close", json!({"windowId": win["windowId"]})).await;
    c.ok("workspace.close", json!({"name": "x"})).await;
    let events = c.notifications(6).await;
    assert_eq!(
        types(&events),
        [
            "workspace.created",
            "workspace.renamed",
            "window.created",
            "window.renamed",
            "window.closed",
            "workspace.closed"
        ]
    );
    assert_eq!(seqs(&events), [2, 3, 4, 5, 6, 7]);
    assert_eq!(events[1]["params"]["data"], json!({"name": "w", "newName": "x"}));
    assert_eq!(events[2]["params"]["data"]["windowId"], win["windowId"]);
}

#[tokio::test]
async fn closing_a_workspace_reports_each_window_before_the_workspace() {
    let mut c = connect(&core());
    c.ok("events.subscribe", json!({})).await;
    c.ok("workspace.create", json!({"name": "w"})).await;
    let a = c
        .ok("window.create", json!({"workspace": "w", "name": "a"}))
        .await;
    let b = c
        .ok("window.create", json!({"workspace": "w", "name": "b"}))
        .await;
    c.ok("workspace.close", json!({"name": "w"})).await;
    let events = c.notifications(5).await;
    assert_eq!(
        types(&events),
        [
            "workspace.created",
            "window.created",
            "window.created",
            "window.closed",
            "window.closed"
        ]
    );
    let closed: Vec<_> = events[3..]
        .iter()
        .map(|e| e["params"]["data"]["windowId"].clone())
        .collect();
    assert_eq!(closed, [a["windowId"].clone(), b["windowId"].clone()]);
    let last = c.next_notification().await;
    assert_eq!(last["params"]["type"], "workspace.closed");
}

#[tokio::test]
async fn pane_lifecycle_events_carry_the_pane_id_outside_data_and_never_the_input() {
    let mut c = connect(&core());
    c.ok("events.subscribe", json!({})).await;
    let pane = c
        .ok(
            "pane.open",
            json!({"cmd": sh("read x; exit 7"), "labels": {"k": "v"}}),
        )
        .await;
    let pane_id = pane["paneId"].as_str().unwrap();
    c.ok(
        "pane.resize",
        json!({"paneId": pane_id, "cols": 70, "rows": 20, "clientId": "A"}),
    )
    .await;
    c.ok(
        "pane.set_label",
        json!({"paneId": pane_id, "set": {"n": "1"}, "unset": ["k"]}),
    )
    .await;
    c.ok(
        "pane.write",
        json!({"paneId": pane_id, "data": "secret-text\n", "source": "terminal"}),
    )
    .await;
    let events = c.notifications(8).await;
    assert_eq!(
        types(&events),
        [
            "workspace.created",
            "window.created",
            "pane.opened",
            "pane.resized",
            "pane.label",
            "input",
            "pane.exited",
            "pane.state",
        ],
        "pane.opened comes after the default workspace / window are announced"
    );
    let by_type = |t: &str| events.iter().find(|e| e["params"]["type"] == t).unwrap()["params"].clone();
    let opened = by_type("pane.opened");
    assert_eq!(opened["paneId"], pane_id);
    assert_eq!(opened["data"]["workspace"], "default");
    assert_eq!(opened["data"]["labels"], json!({"k": "v"}));
    assert_eq!(opened["data"]["cmd"], json!(["sh", "-c", "read x; exit 7"]));
    assert!(opened["data"]["pid"].as_u64().unwrap() > 0);
    assert_eq!(
        by_type("pane.resized")["data"],
        json!({"cols": 70, "rows": 20, "clientId": "A"})
    );
    assert_eq!(
        by_type("pane.label")["data"],
        json!({"set": {"n": "1"}, "unset": ["k"]})
    );
    let input = by_type("input");
    assert_eq!(input["data"], json!({"source": "terminal", "bytes": 12}));
    assert!(!input.to_string().contains("secret-text"));
    assert_eq!(
        by_type("pane.exited")["data"],
        json!({"exitCode": 7, "signal": null})
    );
    assert_eq!(by_type("pane.exited")["paneId"], pane_id);

    assert_eq!(
        by_type("pane.state")["data"],
        json!({"state": "exited", "decidedBy": "exit", "prev": "unknown"})
    );
    c.ok("pane.close", json!({"paneId": pane_id})).await;
    let closed = c.next_notification().await;
    assert_eq!(
        (
            closed["params"]["type"].clone(),
            closed["params"]["paneId"].clone()
        ),
        (json!("pane.closed"), json!(pane_id))
    );
    assert_eq!(closed["params"]["data"], json!({}));
}

#[tokio::test]
async fn a_new_connection_resumes_from_since_without_gap_or_duplicates() {
    let core = core();
    let mut first = connect(&core);
    let epoch = epoch(&mut first).await;
    first
        .ok("events.subscribe", json!({"since": 0, "epoch": epoch}))
        .await;
    first.ok("workspace.create", json!({"name": "a"})).await;
    let seen = first.notifications(2).await;
    let last = *seqs(&seen).last().unwrap();
    drop(first);

    let mut other = connect(&core);
    other.ok("workspace.create", json!({"name": "b"})).await;
    other.ok("workspace.create", json!({"name": "c"})).await;

    let mut resumed = connect(&core);
    let r = resumed
        .ok("events.subscribe", json!({"since": last, "epoch": epoch}))
        .await;
    assert_eq!(r["gap"], false);
    let events = resumed.notifications(2).await;
    assert_eq!(seqs(&events), [last + 1, last + 2]);
    assert_eq!(types(&events), ["workspace.created", "workspace.created"]);
    resumed.assert_no_notification(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn gap_is_reported_when_since_is_evicted_ahead_of_head_or_from_another_epoch() {
    let limits = Limits {
        events: 3,
        ..Limits::default()
    };
    let mut c = connect(&core_with(limits));
    let epoch = epoch(&mut c).await;
    for name in ["a", "b", "c", "d"] {
        c.ok("workspace.create", json!({"name": name})).await;
    }
    // 発行は 5 件（daemon.started + 4）。保持は 3..=5。
    let r = c
        .ok("events.subscribe", json!({"since": 2, "epoch": epoch}))
        .await;
    assert_eq!((r["gap"].clone(), r["head"].clone()), (json!(false), json!(5)));
    assert_eq!(seqs(&c.notifications(3).await), [3, 4, 5]);

    let r = c
        .ok("events.subscribe", json!({"since": 1, "epoch": epoch}))
        .await;
    assert_eq!(r["gap"], true);
    assert_eq!(
        seqs(&c.notifications(3).await),
        [3, 4, 5],
        "replay from the oldest retained event"
    );

    let r = c
        .ok("events.subscribe", json!({"since": 99, "epoch": epoch}))
        .await;
    assert_eq!(r["gap"], true);
    c.assert_no_notification(Duration::from_millis(200)).await;

    let r = c
        .ok("events.subscribe", json!({"since": 5, "epoch": "0".repeat(26)}))
        .await;
    assert_eq!((r["gap"].clone(), r["epoch"].clone()), (json!(true), epoch));
    assert_eq!(seqs(&c.notifications(3).await), [3, 4, 5]);
}

#[tokio::test]
async fn one_events_stream_per_connection_a_second_subscribe_replaces_the_first() {
    let mut c = connect(&core());
    c.ok("events.subscribe", json!({})).await;
    c.ok("events.subscribe", json!({})).await;
    c.ok("workspace.create", json!({"name": "a"})).await;
    let events = c.notifications(1).await;
    assert_eq!(types(&events), ["workspace.created"]);
    c.assert_no_notification(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn independent_connections_each_get_their_own_stream() {
    let core = core();
    let (mut a, mut b) = (connect(&core), connect(&core));
    a.ok("events.subscribe", json!({})).await;
    b.ok("events.subscribe", json!({})).await;
    a.ok("workspace.create", json!({"name": "shared"})).await;
    assert_eq!(types(&a.notifications(1).await), ["workspace.created"]);
    assert_eq!(types(&b.notifications(1).await), ["workspace.created"]);
}

#[tokio::test]
async fn the_response_comes_before_the_first_notification() {
    let mut c = connect(&core());
    c.send_raw(r#"{"jsonrpc":"2.0","id":1,"method":"events.subscribe","params":{"since":0}}"#)
        .await;
    let first = c.read_message().await;
    assert_eq!(
        first["id"], 1,
        "the subscribe result must precede replayed events: {first}"
    );
    assert_eq!(c.read_message().await["method"], "event");
}
