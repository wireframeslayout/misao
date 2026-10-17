//! Node 版との差分テスト。`scripts/gen-rust-diff-fixtures.mjs` が Node 版（xterm + StateTracker + foreground）で作った
//! 記録（`tests/fixtures/node-diff/*.json`）を、Rust 版の `PaneView` に同じ時刻・同じ順序で流し、画面・タイトル・
//! 状態遷移・判定の読み出しが一致することを確かめる。
//!
//! 画面の文字・カーソル・タイトル・activity、状態遷移の内容と時刻、判定の読み出しの全部に、完全な一致を求める。

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::foreground::{command_name_from_argv, parse_foreground_pgid, sanitize_command_name};
use crate::pane_view::{PaneView, ViewEvent};
use crate::profile::{builtin_profiles, select_profile};

/// 記録の置き場所。`MISAO_NODE_DIFF_DIR` で別の場所（調査用に多めに作ったもの）へ向けられる。
fn fixture_dir() -> PathBuf {
    std::env::var_os("MISAO_NODE_DIFF_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/node-diff"),
        PathBuf::from,
    )
}

fn fixture(name: &str) -> Value {
    let path = fixture_dir().join(format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn u64_of(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is not a number in {value}"))
}

fn event_entries(events: Vec<ViewEvent>, at: u64, log: &mut Vec<Value>) {
    for event in events {
        log.push(match event {
            ViewEvent::Title(title) => json!({"kind": "title", "at": at, "title": title}),
            ViewEvent::State(change) => json!({
                "kind": "state", "at": at, "state": change.state.as_str(),
                "decidedBy": change.decided_by, "prev": change.prev.as_str(),
            }),
        });
    }
}

/// 期限が `target` 以前のものを、期限の時刻ごとに実行する（Node 側の仮想時計と同じ進め方）。
fn run_until(view: &mut PaneView, target: u64, log: &mut Vec<Value>) {
    while let Some(deadline) = view.next_deadline().filter(|&d| d <= target) {
        let events = view.advance(deadline);
        event_entries(events, deadline, log);
    }
}

fn step_bytes(step: &Value) -> Vec<u8> {
    match step["bytes"].as_array() {
        Some(bytes) => bytes
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap_or(0)).unwrap_or(0))
            .collect(),
        None => step["data"].as_str().unwrap_or_default().as_bytes().to_vec(),
    }
}

fn replay(scenario: &Value) -> (Option<String>, Vec<Value>) {
    let profiles = builtin_profiles().unwrap_or_default();
    let cmd: Vec<String> = scenario["cmd"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let profile = select_profile(&profiles, &cmd);
    let selected = profile.as_ref().map(|p| p.name.clone());
    let scrollback = usize::try_from(u64_of(scenario, "scrollback")).unwrap_or(0);
    let mut view = PaneView::new(
        u64_of(scenario, "cols"),
        u64_of(scenario, "rows"),
        scrollback,
        profile,
    );
    let mut log = Vec::new();
    for step in scenario["steps"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let at = u64_of(step, "at");
        run_until(&mut view, at, &mut log);
        match step["op"].as_str().unwrap_or_default() {
            "output" => {
                let events = view.output(&step_bytes(step), at);
                event_entries(events, at, &mut log);
            }
            "input" => view.input(at),
            "resize" => view.resize(u64_of(step, "cols"), u64_of(step, "rows"), at),
            "screen" => {
                let s = view.snapshot();
                log.push(json!({
                    "kind": "screen", "at": at, "text": s.text, "snapshot": view.serialize_snapshot(), "cursor": {"x": s.cursor.x, "y": s.cursor.y},
                    "altScreen": s.alt_screen, "title": s.title, "activity": s.activity,
                }));
            }
            "info" => {
                let (state, decided_by) = view.agent_state();
                log.push(
                    json!({"kind": "info", "at": at, "agentState": state.as_str(), "decidedBy": decided_by}),
                );
            }
            "exit" => {
                let events = view.exited();
                event_entries(events, at, &mut log);
            }
            other => panic!("unknown op {other}"),
        }
    }
    run_until(&mut view, u64_of(scenario, "end"), &mut log);
    (selected, log)
}

/// 最初に食い違った項目を報告する。
fn compare(name: &str, expected: &[Value], actual: &[Value]) -> Result<(), String> {
    for (i, (want, got)) in expected.iter().zip(actual).enumerate() {
        if want != got {
            return Err(format!(
                "{name}: log entry #{i} differs\nexpected: {want}\nactual:   {got}"
            ));
        }
    }
    if expected.len() == actual.len() {
        return Ok(());
    }
    Err(format!(
        "{name}: log lengths differ (the first {} entries match): expected {}, got {}",
        expected.len().min(actual.len()),
        expected.len(),
        actual.len()
    ))
}

fn check_scenario(name: &str) -> Result<(), String> {
    let fixture = fixture(name);
    let (profile, log) = replay(&fixture);
    let expect = &fixture["expect"];
    let want_profile = expect["profile"].as_str().map(str::to_string);
    if want_profile != profile {
        return Err(format!(
            "{name}: selected profile: expected {want_profile:?}, got {profile:?}"
        ));
    }
    compare(
        name,
        expect["log"].as_array().map(Vec::as_slice).unwrap_or_default(),
        &log,
    )
}

/// 記録の置き場所にある全シナリオ（`foreground` 以外）の名前。
fn scenario_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixture_dir())
        .unwrap_or_else(|e| panic!("{}: {e}", fixture_dir().display()))
        .filter_map(|entry| entry.ok()?.path().file_stem()?.to_str().map(str::to_string))
        .filter(|name| name != "foreground")
        .collect();
    names.sort();
    names
}

#[test]
fn every_scenario_agrees_with_the_node_implementation() {
    let names = scenario_names();
    assert!(names.len() >= 10, "fixtures are missing: {names:?}");
    let failures: Vec<String> = names
        .iter()
        .filter_map(|name| check_scenario(name).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} scenarios differ:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n\n")
    );
}

#[test]
fn foreground_helpers_agree_with_the_node_implementation() {
    let fixture = fixture("foreground");
    for case in fixture["argv"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let argv: Vec<String> = case["argv"]
            .as_array()
            .map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        assert_eq!(
            command_name_from_argv(&argv),
            case["name"].as_str().map(str::to_string),
            "argv {argv:?}"
        );
    }
    for case in fixture["stats"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let stat = case["stat"].as_str().unwrap_or_default();
        assert_eq!(
            parse_foreground_pgid(stat).map(u64::from),
            case["pgid"].as_u64(),
            "stat {stat:?}"
        );
    }
    for case in fixture["sanitize"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let name = case["name"].as_str().unwrap_or_default();
        assert_eq!(
            sanitize_command_name(name),
            case["sanitized"].as_str().map(str::to_string),
            "name {name:?}"
        );
    }
}
