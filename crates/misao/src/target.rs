//! 対象指定（pane ID / task:N / agent:ID / 窓番号 / ID の前方・後方一致 / 名前の部分一致）から pane を 1 つ決める。

use crate::error::{Candidate, CliError, ErrorKind};
use crate::model::PaneInfo;
use crate::view::pane_view::{
    MIN_ID_FRAGMENT_LENGTH, bare_id, display_name, format_window_id, is_window_number_query,
    strip_window_prefix,
};

/// help に載せる対象指定の書き方。数字だけの指定は窓番号として扱う。
pub const TARGET_HELP: [&str; 6] = [
    "対象 <target> の書き方:",
    "  806 / W-806     窓番号 (数字だけの指定は窓番号として扱う)",
    "  p_01M3…, 7Q     pane ID、または ID の前方・後方一致 (2 文字以上)。数字だけの ID は p_ を付けて書く (p_12)",
    "  task:N          task ラベル",
    "  agent:ID        agent ラベル",
    "  名前の一部      name ラベルか窓名の部分一致",
];

type Matcher<'a> = Box<dyn Fn(&PaneInfo) -> bool + 'a>;

fn label_match<'a>(key: &'static str, value: &'a str) -> Matcher<'a> {
    let want = value.strip_prefix('#').unwrap_or(value).to_string();
    Box::new(move |p| {
        p.labels
            .get(key)
            .is_some_and(|label| label.strip_prefix('#').unwrap_or(label) == want)
    })
}

/// 段階ごとに照合し、最初に 1 件以上当たった段階で決める。
fn stages(query: &str) -> Vec<Matcher<'_>> {
    let upper = query.to_uppercase();
    let bare_query = upper.strip_prefix("P_").unwrap_or(&upper).to_string();
    let lower = query.to_lowercase();
    let mut list: Vec<Matcher<'_>> = vec![Box::new(move |p| p.pane_id == query)];
    if let Some(task) = query.strip_prefix("task:") {
        list.push(label_match("task", task));
    }
    if let Some(agent) = query.strip_prefix("agent:") {
        list.push(label_match("agent", agent));
    }
    let bare_window = strip_window_prefix(query).to_uppercase();
    list.push(Box::new(move |p| {
        p.labels
            .get("windowId")
            .is_some_and(|id| !id.is_empty() && strip_window_prefix(id).to_uppercase() == bare_window)
    }));
    // 窓番号の形（806 / W-806）は窓番号の段だけで決める。閉じた窓の番号が ID や名前の部分一致で別のペインに当たらないようにする。
    if is_window_number_query(query) {
        return list;
    }
    if bare_query.chars().count() >= MIN_ID_FRAGMENT_LENGTH {
        list.push(Box::new(move |p| {
            let bare = bare_id(&p.pane_id).to_uppercase();
            bare.starts_with(&bare_query) || bare.ends_with(&bare_query)
        }));
    }
    list.push(Box::new(move |p| {
        let window_name = &p.window.name;
        [p.labels.get("name"), Some(window_name)]
            .into_iter()
            .flatten()
            .filter(|n| !n.is_empty())
            .any(|n| n.to_lowercase().contains(&lower))
    }));
    list
}

/// 0 件は not_found、同じ段階で複数なら ambiguous（候補付き）。
pub fn resolve_target(query: &str, panes: &[PaneInfo], home_dir: &str) -> Result<PaneInfo, CliError> {
    if query.is_empty() {
        return Err(CliError::usage("対象を指定してください"));
    }
    for matches in stages(query) {
        let hits: Vec<&PaneInfo> = panes.iter().filter(|p| matches(p)).collect();
        match hits.as_slice() {
            [] => {}
            [one] => return Ok((*one).clone()),
            many => {
                let candidates = many
                    .iter()
                    .map(|p| Candidate {
                        pane_id: p.pane_id.clone(),
                        name: display_name(p, home_dir),
                    })
                    .collect();
                return Err(CliError::new(
                    ErrorKind::Ambiguous,
                    format!("\"{query}\" に当てはまるペインが複数あります。絞り込んでください"),
                )
                .with_candidates(candidates));
            }
        }
    }
    if is_window_number_query(query) {
        return Err(CliError::new(
            ErrorKind::NotFound,
            format!(
                "{} は見つかりません。misao ls で確認してください（数字だけの pane ID は p_ を付けて指定します）",
                format_window_id(strip_window_prefix(query))
            ),
        ));
    }
    Err(CliError::new(
        ErrorKind::NotFound,
        format!("\"{query}\" に当てはまるペインがありません"),
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::testing::make_pane;

    const HOME: &str = "/home/test";

    fn pane(id: &str, labels: serde_json::Value) -> PaneInfo {
        make_pane(|v| {
            v["paneId"] = json!(id);
            v["labels"] = labels;
        })
    }

    fn fixtures() -> Vec<PaneInfo> {
        let a = pane(
            "p_01M3XXXXXXXXXXXXXXXXXXXX7Q",
            json!({"windowId": "806", "name": "misao 計画", "task": "419", "agent": "claude-1"}),
        );
        let b = pane(
            "p_01M3XXXXXXXXXXXXXXXXXXXXR8",
            json!({"windowId": "807", "name": "misao 実装", "task": "#437"}),
        );
        let c = make_pane(|v| {
            v["paneId"] = json!("p_07ZZYYYYYYYYYYYYYYYYYYYYK1");
            v["window"] = json!({"id": "w_01M3AAAAAAAAAAAAAAAAAAAAAA", "name": "scratch"});
        });
        vec![a, b, c]
    }

    fn id_of(query: &str, panes: &[PaneInfo]) -> String {
        resolve_target(query, panes, HOME).map_or_else(|e| format!("ERR {}", e.message), |p| p.pane_id)
    }

    #[test]
    fn exact_id_wins_and_fragments_match_either_end_ignoring_case() {
        let panes = fixtures();
        assert_eq!(id_of(&panes[0].pane_id, &panes), panes[0].pane_id);
        for (query, want) in [("07zzy", 2), ("p_07zz", 2), ("r8", 1), ("K1", 2)] {
            assert_eq!(id_of(query, &panes), panes[want].pane_id, "{query}");
        }
    }

    #[test]
    fn labels_window_numbers_and_names() {
        let panes = fixtures();
        for (query, want) in [
            ("task:419", 0),
            ("task:437", 1),
            ("task:#437", 1),
            ("agent:claude-1", 0),
            ("806", 0),
            ("W-807", 1),
            ("w-807", 1),
            ("計画", 0),
            ("SCRATCH", 2),
        ] {
            assert_eq!(id_of(query, &panes), panes[want].pane_id, "{query}");
        }
    }

    #[test]
    fn an_earlier_stage_wins_over_id_matches() {
        let d = pane("p_01M3XXXXXXXXXXXXXXXXXXXX88", json!({}));
        let e = pane("p_01M3XXXXXXXXXXXXXXXXXXXX99", json!({"windowId": "88"}));
        assert_eq!(id_of("88", &[d, e.clone()]), e.pane_id);
    }

    #[test]
    fn zero_hits_is_not_found_and_many_is_ambiguous_with_candidates() {
        let panes = fixtures();
        assert_eq!(
            resolve_target("nothing-here", &panes, HOME).unwrap_err().kind,
            ErrorKind::NotFound
        );
        let err = resolve_target("misao", &panes, HOME).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Ambiguous);
        let candidates = err.candidates.unwrap();
        assert_eq!(
            candidates.iter().map(|c| c.pane_id.as_str()).collect::<Vec<_>>(),
            vec![panes[0].pane_id.as_str(), panes[1].pane_id.as_str()]
        );
        assert_eq!(candidates[0].name, "W-806 · misao 計画");
        assert_eq!(
            resolve_target("", &panes, HOME).unwrap_err().kind,
            ErrorKind::Usage
        );
    }

    #[test]
    fn window_number_queries_do_not_fall_through_to_ids_or_names() {
        let by_id = pane("p_01M3XXXXXXXXXXXXXXXXXXX806", json!({}));
        let by_name = pane("p_01M3XXXXXXXXXXXXXXXXXXXXKK", json!({"name": "issue-806-fix"}));
        let panes = [by_id.clone(), by_name];
        let err = resolve_target("806", &panes, HOME).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(
            err.message
                .starts_with("W-806 は見つかりません。misao ls で確認してください")
        );
        assert!(err.message.contains("p_ を付けて"));
        assert_eq!(
            resolve_target("W-806", &panes, HOME).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert_eq!(id_of("p_806", &panes), by_id.pane_id);
    }

    #[test]
    fn id_fragments_need_two_characters() {
        let only = pane("p_01M3XXXXXXXXXXXXXXXXXXXXQZ", json!({}));
        let panes = [only.clone()];
        assert!(resolve_target("Z", &panes, HOME).is_err());
        assert!(resolve_target("p_0", &panes, HOME).is_err());
        assert_eq!(id_of("QZ", &panes), only.pane_id);
    }
}
