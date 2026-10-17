//! ペインの並び順・表示名・短縮 ID（Node 版の `view/pane-view.ts`）。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::model::PaneInfo;

/// 一覧での並び順と表示に使う状態。processState が stopped のものは stopped 扱い。
const STATE_ORDER: [&str; 6] = ["blocked", "working", "idle", "exited", "stopped", "unknown"];

const NO_VALUE: &str = "—";
const ID_PREFIX_LENGTH: usize = 4;
/// ID の前方・後方一致に使う最短の長さ。短縮 ID の末尾もこの長さ以上にする（照合と表示で共有）。
pub const MIN_ID_FRAGMENT_LENGTH: usize = 2;

pub fn pane_state_key(pane: &PaneInfo) -> &'static str {
    if pane.process_state == "stopped" {
        return "stopped";
    }
    STATE_ORDER
        .iter()
        .copied()
        .find(|s| *s == pane.agent_state && *s != "stopped")
        .unwrap_or("unknown")
}

fn state_symbol(key: &str) -> &'static str {
    match key {
        "blocked" => "●",
        "working" => "◐",
        "idle" => "○",
        "exited" => "✕",
        "stopped" => "■",
        _ => "?",
    }
}

pub fn state_label(pane: &PaneInfo) -> String {
    let key = pane_state_key(pane);
    format!("{} {key}", state_symbol(key))
}

/// ISO 8601 の時刻をミリ秒にする。読めなければ None。
pub fn parse_time_ms(iso: &str) -> Option<i64> {
    let parsed = time::OffsetDateTime::parse(iso, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

fn order_index(pane: &PaneInfo) -> usize {
    let key = pane_state_key(pane);
    STATE_ORDER
        .iter()
        .position(|s| *s == key)
        .unwrap_or(STATE_ORDER.len())
}

/// blocked → working → idle → exited → stopped → unknown。同状態の中は lastOutputAt の新しい順。
pub fn sort_panes(panes: &[PaneInfo]) -> Vec<PaneInfo> {
    let time_of = |p: &PaneInfo| {
        p.last_output_at
            .as_deref()
            .and_then(parse_time_ms)
            .unwrap_or(i64::MIN)
    };
    let mut sorted = panes.to_vec();
    sorted.sort_by(|a, b| {
        order_index(a)
            .cmp(&order_index(b))
            .then_with(|| match time_of(b).cmp(&time_of(a)) {
                Ordering::Equal => a.pane_id.cmp(&b.pane_id),
                other => other,
            })
    });
    sorted
}

/// hub に登録された窓か（windowId ラベルがある）。
pub fn is_registered(pane: &PaneInfo) -> bool {
    pane.labels.get("windowId").is_some_and(|id| !id.is_empty())
}

/// 窓番号の先頭の w- / W- を除く。
pub fn strip_window_prefix(window_id: &str) -> &str {
    match window_id.get(..2) {
        Some(prefix) if prefix.eq_ignore_ascii_case("w-") => &window_id[2..],
        _ => window_id,
    }
}

/// hub 側の窓番号 806 を W-806 と表示する。すでに W- が付いていればそのまま。
pub fn format_window_id(window_id: &str) -> String {
    format!("W-{}", strip_window_prefix(window_id))
}

/// 窓番号の形（806 / W-806）の対象指定か。
pub fn is_window_number_query(query: &str) -> bool {
    let digits = strip_window_prefix(query);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// ホームディレクトリ配下を ~ に短縮する。
pub fn tildify(cwd: &str, home_dir: &str) -> String {
    if cwd == home_dir {
        return "~".to_string();
    }
    match cwd.strip_prefix(home_dir) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => cwd.to_string(),
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// 前面コマンド。取れていなければ起動コマンドの basename。
pub fn foreground_command(pane: &PaneInfo) -> String {
    match pane.fg_command.as_deref() {
        Some(fg) if !fg.is_empty() => fg.to_string(),
        _ => basename(pane.cmd.first().map_or("", String::as_str)).to_string(),
    }
}

/// NAME 列の表示。
/// 登録済み: `W-806 · 表示名` / `W-806`。未登録: `(未登録) <name>`、name も無ければ `(未登録) <前面コマンド> · <cwd>`。
pub fn display_name(pane: &PaneInfo, home_dir: &str) -> String {
    let name = pane.labels.get("name").filter(|n| !n.is_empty());
    if is_registered(pane) {
        let id = format_window_id(pane.labels.get("windowId").map_or("", String::as_str));
        return name.map_or(id.clone(), |n| format!("{id} · {n}"));
    }
    match name {
        Some(n) => format!("(未登録) {n}"),
        None => format!(
            "(未登録) {} · {}",
            foreground_command(pane),
            tildify(&pane.cwd, home_dir)
        ),
    }
}

/// task ラベルの値から先頭の # を除く。
pub fn task_number(pane: &PaneInfo) -> Option<String> {
    let task = pane.labels.get("task").filter(|t| !t.is_empty())?;
    Some(task.strip_prefix('#').unwrap_or(task).to_string())
}

pub fn task_label(pane: &PaneInfo) -> String {
    task_number(pane).map_or(NO_VALUE.to_string(), |t| format!("#{t}"))
}

/// lastOutputAt からの相対時間（12s / 4m / 2h）。無ければ —。
pub fn relative_time(iso: Option<&str>, now_ms: i64) -> String {
    let Some(at) = iso.and_then(parse_time_ms) else {
        return NO_VALUE.to_string();
    };
    // JavaScript の Math.round は .5 を切り上げる。
    let seconds = (((now_ms - at) as f64) / 1000.0 + 0.5).floor().max(0.0) as i64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

/// `p_01M3…7Q` 形式（先頭 4 文字 + … + 一意になる最短の末尾、最小 2）。
/// 末尾は単独で対象指定（`misao attach 7Q`）に使うので、resolveTarget で他の pane に当たらない長さまで伸ばす:
/// 他の ID の先頭・末尾（大文字小文字を区別しない）と一致せず、窓番号の段に吸われる形（数字だけ、
/// またはいずれかの pane の windowId と等しい）でもない長さ。
pub fn short_pane_ids(panes: &[PaneInfo]) -> HashMap<String, String> {
    let bodies: Vec<String> = panes.iter().map(|p| bare_id(&p.pane_id).to_uppercase()).collect();
    let window_ids: HashSet<String> = panes
        .iter()
        .filter_map(|p| p.labels.get("windowId"))
        .filter(|w| !w.is_empty())
        .map(|w| strip_window_prefix(w).to_uppercase())
        .collect();
    let mut result = HashMap::new();
    for (index, pane) in panes.iter().enumerate() {
        let body: Vec<char> = bare_id(&pane.pane_id).chars().collect();
        let is_unusable = |suffix: &str| {
            is_window_number_query(suffix)
                || window_ids.contains(suffix)
                || bodies
                    .iter()
                    .enumerate()
                    .any(|(i, o)| i != index && (o.starts_with(suffix) || o.ends_with(suffix)))
        };
        let tail = |n: usize| body[body.len().saturating_sub(n)..].iter().collect::<String>();
        let mut length = MIN_ID_FRAGMENT_LENGTH;
        while length < body.len() && is_unusable(&tail(length).to_uppercase()) {
            length += 1;
        }
        let head: String = body.iter().take(ID_PREFIX_LENGTH).collect();
        result.insert(pane.pane_id.clone(), format!("p_{head}…{}", tail(length)));
    }
    result
}

/// `p_` を除いた ID 本体。
pub fn bare_id(pane_id: &str) -> &str {
    pane_id.get(2..).unwrap_or("")
}

/// short_pane_ids の末尾部分（`p_01M3…R8` の R8）。attach / kill の対象指定にそのまま使える。
pub fn short_suffix(short_id: &str) -> &str {
    short_id.split_once('…').map_or(short_id, |(_, tail)| tail)
}

/// 「W-806 から抜けました」のような文中の呼び名。登録済みなら窓番号だけ、未登録なら NAME 列と同じ。
pub fn short_display_name(pane: &PaneInfo, home_dir: &str) -> String {
    if is_registered(pane) {
        format_window_id(pane.labels.get("windowId").map_or("", String::as_str))
    } else {
        display_name(pane, home_dir)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::testing::make_pane;

    const HOME: &str = "/home/test";

    fn with_id(id: &str) -> PaneInfo {
        make_pane(|v| v["paneId"] = json!(id))
    }

    fn ids(panes: &[&str]) -> Vec<PaneInfo> {
        panes.iter().map(|p| with_id(p)).collect()
    }

    fn t(seconds: i64) -> String {
        let at = time::OffsetDateTime::from_unix_timestamp(1_767_225_600 + seconds).unwrap();
        at.format(&time::format_description::well_known::Rfc3339).unwrap()
    }

    #[test]
    fn sorts_by_state_then_newest_and_does_not_touch_the_input() {
        let mk = |agent: &str, process: &str, at: Option<String>| {
            make_pane(|v| {
                v["agentState"] = json!(agent);
                v["processState"] = json!(process);
                v["lastOutputAt"] = json!(at);
            })
        };
        let panes = vec![
            mk("idle", "running", Some(t(5))),
            mk("unknown", "running", Some(t(50))),
            mk("working", "running", Some(t(1))),
            mk("working", "running", Some(t(9))),
            mk("working", "running", None),
            mk("blocked", "running", Some(t(2))),
            mk("exited", "exited", Some(t(3))),
            mk("idle", "stopped", Some(t(40))),
        ];
        let sorted = sort_panes(&panes);
        let got: Vec<(&str, Option<String>)> = sorted
            .iter()
            .map(|p| (pane_state_key(p), p.last_output_at.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("blocked", Some(t(2))),
                ("working", Some(t(9))),
                ("working", Some(t(1))),
                ("working", None),
                ("idle", Some(t(5))),
                ("exited", Some(t(3))),
                ("stopped", Some(t(40))),
                ("unknown", Some(t(50))),
            ]
        );
        assert_eq!(panes[0].agent_state, "idle");
    }

    #[test]
    fn state_labels_carry_a_symbol() {
        let label = |agent: &str, process: &str| {
            state_label(&make_pane(|v| {
                v["agentState"] = json!(agent);
                v["processState"] = json!(process);
            }))
        };
        assert_eq!(label("blocked", "running"), "● blocked");
        assert_eq!(label("working", "running"), "◐ working");
        assert_eq!(label("idle", "running"), "○ idle");
        assert_eq!(label("exited", "exited"), "✕ exited");
        assert_eq!(label("idle", "stopped"), "■ stopped");
        assert_eq!(label("brand-new-state", "running"), "? unknown");
    }

    #[test]
    fn display_names_cover_registered_and_unregistered() {
        let name = |edit: &dyn Fn(&mut serde_json::Value)| display_name(&make_pane(edit), HOME);
        assert_eq!(
            name(&|v| v["labels"] = json!({"windowId": "806", "name": "表示名"})),
            "W-806 · 表示名"
        );
        assert_eq!(name(&|v| v["labels"] = json!({"windowId": "806"})), "W-806");
        assert_eq!(name(&|v| v["labels"] = json!({"name": "メモ"})), "(未登録) メモ");
        assert_eq!(
            name(&|v| {
                v["cmd"] = json!(["/usr/bin/claude"]);
                v["cwd"] = json!("/home/test/workspace/azito");
                v["fgCommand"] = json!("node");
            }),
            "(未登録) node · ~/workspace/azito"
        );
        assert_eq!(
            name(&|v| {
                v["cmd"] = json!(["/usr/bin/claude"]);
                v["cwd"] = json!("/srv/x");
            }),
            "(未登録) claude · /srv/x"
        );
    }

    #[test]
    fn registration_and_window_ids() {
        let registered = |labels: serde_json::Value| is_registered(&make_pane(|v| v["labels"] = labels));
        assert!(registered(json!({"windowId": "806"})));
        assert!(!registered(json!({"windowId": ""})));
        assert!(!registered(json!({})));
        assert_eq!(format_window_id("806"), "W-806");
        assert_eq!(format_window_id("w-806"), "W-806");
        assert!(
            is_window_number_query("W-806")
                && is_window_number_query("806")
                && !is_window_number_query("p_806")
        );
    }

    #[test]
    fn home_foreground_and_task_helpers() {
        assert_eq!(tildify("/home/test", HOME), "~");
        assert_eq!(tildify("/home/test/a/b", HOME), "~/a/b");
        assert_eq!(tildify("/home/tester/a", HOME), "/home/tester/a");
        assert_eq!(
            foreground_command(&make_pane(|v| v["fgCommand"] = json!("vim"))),
            "vim"
        );
        assert_eq!(
            foreground_command(&make_pane(|v| {
                v["fgCommand"] = json!("");
                v["cmd"] = json!(["/bin/zsh"]);
            })),
            "zsh"
        );
        let task = |label: &str| task_label(&make_pane(|v| v["labels"] = json!({"task": label})));
        assert_eq!(
            (task("419"), task("#419")),
            ("#419".to_string(), "#419".to_string())
        );
        assert_eq!(task_label(&make_pane(|_| {})), "—");
    }

    #[test]
    fn relative_times() {
        let now = parse_time_ms("2026-01-01T12:00:00Z").unwrap();
        let ago = |s: i64| {
            time::OffsetDateTime::from_unix_timestamp(now / 1000 - s)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        };
        assert_eq!(relative_time(None, now), "—");
        assert_eq!(relative_time(Some(&ago(12)), now), "12s");
        assert_eq!(relative_time(Some(&ago(4 * 60 + 5)), now), "4m");
        assert_eq!(relative_time(Some(&ago(2 * 3600 + 100)), now), "2h");
        assert_eq!(relative_time(Some(&ago(-5)), now), "0s");
    }

    #[test]
    fn short_ids_lengthen_the_tail_on_collision() {
        let (a, b, c) = (
            "p_01M3XXXXXXXXXXXXXXXXXXXX7Q",
            "p_01M3XXXXXXXXXXXXXXXXXXXXR8",
            "p_01M3XXXXXXXXXXXXXXXXXXXY7Q",
        );
        let one = short_pane_ids(&ids(&[a, b]));
        assert_eq!((one[a].as_str(), one[b].as_str()), ("p_01M3…7Q", "p_01M3…R8"));
        let three = short_pane_ids(&ids(&[a, b, c]));
        assert_eq!(
            (three[a].as_str(), three[c].as_str(), three[b].as_str()),
            ("p_01M3…X7Q", "p_01M3…Y7Q", "p_01M3…R8")
        );
        assert_eq!(short_suffix("p_01M3…R8"), "R8");
    }

    #[test]
    fn short_tails_do_not_collide_with_other_ids() {
        let (a, b, c) = (
            "p_01M3XXXXXXXXXXXXXXXXXXXXR8",
            "p_01M4YYYYYYYYYYYYYYYYYYYYR8",
            "p_01M5ZZZZZZZZZZZZZZZZZZZZ01",
        );
        let got = short_pane_ids(&ids(&[a, b, c]));
        assert_eq!(
            (got[a].as_str(), got[b].as_str(), got[c].as_str()),
            ("p_01M3…XR8", "p_01M4…YR8", "p_01M5…Z01")
        );
        let lower = short_pane_ids(&ids(&[
            "p_01M3XXXXXXXXXXXXXXXXXXXXab",
            "p_01M4YYYYYYYYYYYYYYYYYYYYAB",
        ]));
        assert_eq!(lower["p_01M3XXXXXXXXXXXXXXXXXXXXab"], "p_01M3…Xab");
    }

    #[test]
    fn short_tails_avoid_window_numbers() {
        let a = "p_01M3XXXXXXXXXXXXXXXXXXXXR8";
        let mut panes = ids(&[a]);
        panes.push(make_pane(|v| {
            v["paneId"] = json!("p_01M4YYYYYYYYYYYYYYYYYYYYQQ");
            v["labels"] = json!({"windowId": "W-r8"});
        }));
        assert_eq!(short_pane_ids(&panes)[a], "p_01M3…XR8");
        let digits = ids(&["p_01M3XXXXXXXXXXXXXXXXXXXK12", "p_01M3XXXXXXXXXXXXXXXXXX9812"]);
        let got = short_pane_ids(&digits);
        assert_eq!(got["p_01M3XXXXXXXXXXXXXXXXXXXK12"], "p_01M3…K12");
        assert_eq!(got["p_01M3XXXXXXXXXXXXXXXXXX9812"], "p_01M3…X9812");
    }

    #[test]
    fn short_display_names() {
        let reg = make_pane(|v| v["labels"] = json!({"windowId": "806", "name": "表示名"}));
        assert_eq!(short_display_name(&reg, HOME), "W-806");
        let unreg = make_pane(|v| v["labels"] = json!({"name": "メモ"}));
        assert_eq!(short_display_name(&unreg, HOME), "(未登録) メモ");
    }
}
