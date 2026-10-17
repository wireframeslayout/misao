//! workspace.* / window.* の RPC（メモリ上の接続で確かめる）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use serde_json::json;

use common::{connect, core};

const WORKSPACE_NOT_FOUND: i64 = 1006;
const WINDOW_NOT_FOUND: i64 = 1007;
const ALREADY_EXISTS: i64 = 1008;
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const NOT_IMPLEMENTED: i64 = 1005;

#[tokio::test]
async fn workspaces_and_windows_are_created_listed_renamed_and_closed() {
    let mut c = connect(&core());
    let ws = c.ok("workspace.create", json!({"name": "dev"})).await;
    assert_eq!(ws, json!({"name": "dev", "windows": []}));
    let win = c
        .ok("window.create", json!({"workspace": "dev", "name": "main"}))
        .await;
    let window_id = win["windowId"].as_str().unwrap().to_string();
    assert!(window_id.starts_with("w_") && window_id.len() == 28);
    assert_eq!(win["name"], "main");
    assert_eq!(win["workspace"], "dev");

    assert_eq!(
        c.ok("window.rename", json!({"windowId": window_id, "name": "renamed"}))
            .await,
        json!({"ok": true})
    );
    assert_eq!(
        c.ok("workspace.rename", json!({"name": "dev", "newName": "prod"}))
            .await,
        json!({"ok": true})
    );
    let list = c.ok("workspace.list", json!({})).await;
    assert_eq!(
        list,
        json!([{"name": "prod", "windows": [{"windowId": window_id, "name": "renamed", "workspace": "prod"}]}])
    );

    assert_eq!(
        c.ok("window.close", json!({"windowId": window_id})).await,
        json!({"ok": true})
    );
    assert_eq!(
        c.ok("workspace.close", json!({"name": "prod"})).await,
        json!({"ok": true})
    );
    assert_eq!(c.ok("workspace.list", json!({})).await, json!([]));
}

#[tokio::test]
async fn errors_use_the_documented_codes() {
    let mut c = connect(&core());
    c.ok("workspace.create", json!({"name": "a"})).await;
    assert_eq!(
        c.err_code("workspace.create", json!({"name": "a"})).await,
        ALREADY_EXISTS
    );
    assert_eq!(
        c.err_code("workspace.close", json!({"name": "none"})).await,
        WORKSPACE_NOT_FOUND
    );
    assert_eq!(
        c.err_code("window.create", json!({"workspace": "none", "name": "w"}))
            .await,
        WORKSPACE_NOT_FOUND
    );
    let missing = format!("w_{}", "0".repeat(26));
    assert_eq!(
        c.err_code("window.close", json!({"windowId": missing})).await,
        WINDOW_NOT_FOUND
    );
    assert_eq!(
        c.err_code("window.rename", json!({"windowId": missing, "name": "x"}))
            .await,
        WINDOW_NOT_FOUND
    );
    assert_eq!(
        c.err_code("workspace.create", json!({"name": ""})).await,
        INVALID_PARAMS
    );
    assert_eq!(
        c.err_code("window.close", json!({"windowId": "w_bad"})).await,
        INVALID_PARAMS
    );
    assert_eq!(c.err_code("no.such.method", json!({})).await, METHOD_NOT_FOUND);
    assert_eq!(c.err_code("window.focus", json!({})).await, NOT_IMPLEMENTED);
}

#[tokio::test]
async fn invalid_lines_get_protocol_level_errors() {
    let mut c = connect(&core());
    c.send_raw("{not json").await;
    let parse = c.read_message().await;
    assert_eq!(parse["error"]["code"], -32700);
    assert!(parse["id"].is_null());
    c.send_raw(r#"{"jsonrpc":"2.0","id":7,"method":5}"#).await;
    let invalid = c.read_message().await;
    assert_eq!(
        (invalid["error"]["code"].as_i64(), invalid["id"].as_i64()),
        (Some(-32600), Some(7))
    );
    // 通知（id なし）は無視され、次のリクエストが普通に答えられる。
    c.send_raw(r#"{"jsonrpc":"2.0","method":"workspace.list"}"#).await;
    assert_eq!(c.ok("workspace.list", json!({})).await, json!([]));
}

#[tokio::test]
async fn server_info_reports_the_event_head_and_pane_count() {
    let mut c = connect(&core());
    let before = c.ok("server.info", json!({})).await;
    assert_eq!(before["paneCount"], 0);
    assert_eq!(before["eventHead"], 1, "daemon.started が 1 件目");
    c.ok("workspace.create", json!({"name": "a"})).await;
    let after = c.ok("server.info", json!({})).await;
    assert_eq!(after["eventHead"], 2);
    assert_eq!(after["epoch"], before["epoch"]);
}
