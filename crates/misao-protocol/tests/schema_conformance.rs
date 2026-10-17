//! `schema/protocol.json`（`@misao/protocol` の Zod から `scripts/export-protocol-schema.mjs` で書き出した
//! JSON Schema）と、Rust の型・定数が食い違わないことを確かめる。

#![allow(clippy::unwrap_used)]

use jsonschema::Validator;
use misao_protocol::{
    ErrorCode, Incoming, METHODS, NOTIFICATIONS, PROTOCOL_VERSION, Response, ServerInfo, parse_incoming,
};
use serde_json::{Value, json};

fn schema() -> Value {
    serde_json::from_str(include_str!("../schema/protocol.json")).unwrap()
}

/// ルートの `$defs` などの参照を解決できるよう、部分スキーマは自己完結のまま検証する。
fn validator(sub: &Value) -> Validator {
    jsonschema::validator_for(sub).unwrap()
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort_unstable();
    v
}

#[test]
fn protocol_version_matches() {
    assert_eq!(schema()["protocolVersion"], PROTOCOL_VERSION);
}

#[test]
fn method_and_notification_names_match() {
    let s = schema();
    // serde_json の Map はキー順を保たないので、順序を無視して比べる。
    assert_eq!(sorted(keys(&s["methods"])), sorted(METHODS.to_vec()));
    assert_eq!(sorted(keys(&s["notifications"])), sorted(NOTIFICATIONS.to_vec()));
}

#[test]
fn error_codes_match() {
    let s = schema();
    let expected: Vec<(String, i64)> = s["errors"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_i64().unwrap()))
        .collect();
    let actual: Vec<(String, i64)> = ErrorCode::ALL
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
    assert_eq!(sorted(actual), sorted(expected));
}

fn sample_info(version: Option<&str>) -> ServerInfo {
    ServerInfo {
        protocol_version: PROTOCOL_VERSION.to_string(),
        version: version.map(str::to_string),
        pid: 4242,
        epoch: "01JABCDEFGHJKMNPQRSTVWXYZ0".to_string(),
        uptime_sec: 3,
        pane_count: 0,
        event_head: 1,
    }
}

#[test]
fn server_info_result_validates_against_schema() {
    let s = schema();
    let v = validator(&s["methods"]["server.info"]["result"]);
    for version in [Some("0.1.0"), None] {
        let value = serde_json::to_value(sample_info(version)).unwrap();
        assert!(
            v.is_valid(&value),
            "{value} does not satisfy the server.info result schema"
        );
    }
}

#[test]
fn server_info_fields_are_exactly_the_schema_properties() {
    let s = schema();
    let result = &s["methods"]["server.info"]["result"];
    let mut schema_props: Vec<&str> = keys(&result["properties"]);
    schema_props.sort_unstable();
    let full = serde_json::to_value(sample_info(Some("0.1.0"))).unwrap();
    let mut rust_props: Vec<&str> = keys(&full);
    rust_props.sort_unstable();
    assert_eq!(rust_props, schema_props);

    let mut required: Vec<&str> = result["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    required.sort_unstable();
    let without_optional = serde_json::to_value(sample_info(None)).unwrap();
    let mut rust_required: Vec<&str> = keys(&without_optional);
    rust_required.sort_unstable();
    assert_eq!(
        rust_required, required,
        "required = the fields serialized when every Option is None"
    );
}

#[test]
fn server_info_params_accept_empty_object() {
    let s = schema();
    assert!(validator(&s["methods"]["server.info"]["params"]).is_valid(&json!({})));
}

#[test]
fn responses_validate_against_jsonrpc_schema() {
    let s = schema();
    let v = validator(&s["jsonrpc"]);
    let info = serde_json::to_value(sample_info(Some("0.1.0"))).unwrap();
    let responses = [
        Response::success(parse_id(json!(1)), info),
        Response::success(parse_id(json!("a")), Value::Null),
        Response::error(
            Some(parse_id(json!(7))),
            ErrorCode::NOT_IMPLEMENTED,
            "not implemented: pane.open",
        ),
        Response::invalid_request(None),
        Response::parse_error("Unexpected token"),
    ];
    for r in responses {
        let value = serde_json::to_value(&r).unwrap();
        assert!(v.is_valid(&value), "{value} does not satisfy the JSON-RPC schema");
    }
}

fn parse_id(v: Value) -> misao_protocol::RequestId {
    serde_json::from_value(v).unwrap()
}

#[test]
fn request_classification_agrees_with_jsonrpc_schema() {
    let s = schema();
    let v = validator(&s["jsonrpc"]);
    let ok = json!({"jsonrpc": "2.0", "id": 1, "method": "server.info", "params": {}});
    assert!(v.is_valid(&ok));
    assert!(matches!(parse_incoming(&ok), Incoming::Request(r) if r.method == "server.info"));

    assert_eq!(parse_incoming(&json!([1])), Incoming::Invalid { id: None });
    assert_eq!(
        parse_incoming(&json!({"jsonrpc": "2.0", "method": "x", "params": {}})),
        Incoming::Ignored
    );
    assert_eq!(
        parse_incoming(&json!({"jsonrpc": "1.0", "id": "z", "method": "x"})),
        Incoming::Invalid {
            id: Some(parse_id(json!("z")))
        }
    );
    assert_eq!(
        parse_incoming(&json!({"jsonrpc": "2.0", "id": {}, "method": "x"})),
        Incoming::Invalid { id: None }
    );
    assert_eq!(
        parse_incoming(&json!({"jsonrpc": "2.0", "id": 2, "method": "x", "params": [1]})),
        Incoming::Invalid {
            id: Some(parse_id(json!(2)))
        }
    );
}
