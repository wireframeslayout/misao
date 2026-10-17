//! デーモンが返すペインの情報（`pane.info` / `pane.list` の要素）。受信側なので未知のフィールドと未知の列挙値は許容する。

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WindowRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneInfo {
    pub pane_id: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub workspace: String,
    pub window: WindowRef,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub process_state: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub signal: Option<i64>,
    pub agent_state: String,
    #[serde(default)]
    pub fg_command: Option<String>,
    #[serde(default)]
    pub last_output_at: Option<String>,
    #[serde(default)]
    pub clients: Vec<String>,
    /// 受け取った JSON そのもの（`--json` で加工せずに出す）。
    #[serde(skip)]
    pub raw: Value,
}

impl PaneInfo {
    pub fn from_value(value: Value) -> Result<Self, String> {
        let mut pane: Self =
            serde_json::from_value(value.clone()).map_err(|e| format!("invalid pane info: {e}"))?;
        pane.raw = value;
        Ok(pane)
    }

    pub fn list_from_value(value: Value) -> Result<Vec<Self>, String> {
        match value {
            Value::Array(items) => items.into_iter().map(Self::from_value).collect(),
            other => Err(format!("invalid pane list: {other}")),
        }
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::PaneInfo;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// 26 文字の ULID 形式になる pane ID。
    pub fn fake_pane_id(tail: &str) -> String {
        let body = format!("01M3{}{tail}", "A".repeat(20 - tail.len()));
        format!("p_{body}")
    }

    pub fn make_pane(edit: impl FnOnce(&mut serde_json::Value)) -> PaneInfo {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        let mut value = json!({
            "paneId": fake_pane_id(&format!("{n:02}")),
            "pid": 100 + n,
            "cmd": ["/bin/zsh", "-l"],
            "cwd": "/home/test/work",
            "workspace": "default",
            "window": {"id": "w_01M3AAAAAAAAAAAAAAAAAAAAAA", "name": "default"},
            "labels": {},
            "processState": "running",
            "exitCode": null,
            "signal": null,
            "agentState": "idle",
            "decidedBy": "bytes",
            "title": "",
            "lastOutputAt": null,
            "cols": 80,
            "rows": 24,
            "clients": [],
            "sizeOwner": null,
        });
        edit(&mut value);
        PaneInfo::from_value(value).unwrap_or_else(|e| panic!("{e}"))
    }
}
