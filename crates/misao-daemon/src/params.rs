//! リクエストの params の検証。正本は `@misao/protocol` の Zod スキーマ（`packages/protocol/src/methods`）で、
//! ここはその制約（空でない名前・ID の形式・正の整数など）を serde で読み替えたもの。
//! 違反は `invalid params for <method>: <詳細>`（-32602）。詳細の文言は Zod のものと一致しない。

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::rpc_error::{RpcFailure, RpcResult};

pub type Labels = BTreeMap<String, String>;

/// 検証済みの文字列（`TryFrom<String>` で形式を確かめる）。
macro_rules! validated_string {
    ($(#[$meta:meta])* $name:ident, $check:expr, $expected:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> Result<Self, String> {
                let check: fn(&str) -> bool = $check;
                if check(&value) {
                    Ok(Self(value))
                } else {
                    Err(format!("invalid value {value:?}: expected {}", $expected))
                }
            }
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

/// Crockford base32 の ULID（128bit なので先頭文字は 0-7）。
fn is_ulid(s: &str) -> bool {
    const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    s.len() == 26
        && s.chars().enumerate().all(|(i, c)| {
            if i == 0 {
                ('0'..='7').contains(&c)
            } else {
                CROCKFORD.contains(c)
            }
        })
}

validated_string!(
    /// workspace 名・window 名・clientId。空でない文字列。
    Name,
    |s| !s.is_empty(),
    "a non-empty string"
);
validated_string!(
    PaneId,
    |s| s.strip_prefix("p_").is_some_and(is_ulid),
    "\"p_\" followed by a ULID"
);
validated_string!(
    WindowId,
    |s| s.strip_prefix("w_").is_some_and(is_ulid),
    "\"w_\" followed by a ULID"
);
validated_string!(Epoch, is_ulid, "a ULID");

/// JSON の整数（`2.0` も整数として受ける。Zod の `z.int()` と同じ）。
#[allow(clippy::cast_possible_truncation)]
fn integer(n: &Number) -> Option<i128> {
    if let Some(v) = n.as_i64() {
        return Some(i128::from(v));
    }
    if let Some(v) = n.as_u64() {
        return Some(i128::from(v));
    }
    let f = n.as_f64()?;
    // 2^53 未満（JavaScript の安全な整数）なら f64 から正確に戻せる。
    if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        Some(f as i128)
    } else {
        None
    }
}

/// JavaScript の安全な整数の上限（Zod の `z.int()` が受ける範囲）。
const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991;

/// 正の整数（cols / rows）。値はそのまま保持し、pty へ渡すときだけ 16 bit に丸める。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Number", into = "u64")]
pub struct Dimension(u64);

impl From<Dimension> for u64 {
    fn from(value: Dimension) -> Self {
        value.0
    }
}

impl TryFrom<Number> for Dimension {
    type Error = String;
    fn try_from(n: Number) -> Result<Self, String> {
        integer(&n)
            .filter(|v| (1..=MAX_SAFE_INTEGER).contains(v))
            .and_then(|v| u64::try_from(v).ok())
            .map(Self)
            .ok_or_else(|| format!("invalid value {n}: expected a positive integer"))
    }
}

impl Dimension {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// seq（0 以上の整数）。`since` に使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Number")]
pub struct Seq(u64);

impl TryFrom<Number> for Seq {
    type Error = String;
    fn try_from(n: Number) -> Result<Self, String> {
        integer(&n)
            .and_then(|v| u64::try_from(v).ok())
            .map(Self)
            .ok_or_else(|| format!("invalid value {n}: expected an integer >= 0"))
    }
}

impl Seq {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSource {
    Hub,
    Terminal,
}

impl InputSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hub => "hub",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessState {
    Running,
    Exited,
    Stopped,
}

#[derive(Debug, Deserialize)]
pub struct Empty {}

#[derive(Debug, Deserialize)]
pub struct WorkspaceNameParams {
    pub name: Name,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRenameParams {
    pub name: Name,
    pub new_name: Name,
}

#[derive(Debug, Deserialize)]
pub struct WindowCreateParams {
    pub workspace: Name,
    pub name: Name,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowIdParams {
    pub window_id: WindowId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRenameParams {
    pub window_id: WindowId,
    pub name: Name,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneRefParams {
    pub pane_id: PaneId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneOpenParams {
    pub cmd: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<BTreeMap<String, String>>,
    pub ephemeral_env: Option<BTreeMap<String, String>>,
    pub cols: Option<Dimension>,
    pub rows: Option<Dimension>,
    pub labels: Option<Labels>,
    pub window_id: Option<WindowId>,
    /// 未実装（NotImplemented を返す）なので、配列であることだけを確かめる。
    pub preplace: Option<Vec<serde::de::IgnoredAny>>,
}

/// 全条件の AND。未知の条件は拒否する（古いデーモンが条件を無視して全 pane を返さないため）。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneListFilter {
    pub state: Option<ProcessState>,
    pub labels: Option<Labels>,
    pub workspace: Option<Name>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachMode {
    #[default]
    Raw,
    Cells,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Replay {
    Raw,
    Snapshot,
    #[default]
    None,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneAttachParams {
    pub pane_id: PaneId,
    pub client_id: Name,
    #[serde(default)]
    pub mode: AttachMode,
    #[serde(default)]
    pub replay: Replay,
    pub cols: Option<Dimension>,
    pub rows: Option<Dimension>,
}

#[derive(Debug, Deserialize)]
pub struct PaneListParams {
    pub filter: Option<PaneListFilter>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneWriteParams {
    pub pane_id: PaneId,
    pub client_id: Option<Name>,
    pub source: Option<InputSource>,
    pub data: Option<String>,
    pub data_b64: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneResizeParams {
    pub pane_id: PaneId,
    pub cols: Dimension,
    pub rows: Dimension,
    pub client_id: Option<Name>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneSetLabelParams {
    pub pane_id: PaneId,
    pub set: Option<Labels>,
    pub unset: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeLinesParams {
    pub pane_id: PaneId,
    pub since: Option<Seq>,
    pub epoch: Option<Epoch>,
}

#[derive(Debug, Deserialize)]
pub struct EventsSubscribeParams {
    pub since: Option<Seq>,
    pub epoch: Option<Epoch>,
}

/// params を型へ読む。params 省略は `{}` と同じ。
pub fn parse<T: DeserializeOwned>(method: &str, params: Option<&Map<String, Value>>) -> RpcResult<T> {
    let value = Value::Object(params.cloned().unwrap_or_default());
    serde_json::from_value(value)
        .map_err(|e| RpcFailure::invalid_params(format!("invalid params for {method}: {e}")))
}

impl PaneWriteParams {
    /// `data`（UTF-8）と `dataB64` はどちらか一方だけ。
    pub fn require_one_payload(&self, method: &str) -> RpcResult<()> {
        match (&self.data, &self.data_b64) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            _ => Err(RpcFailure::invalid_params(format!(
                "invalid params for {method}: exactly one of data and dataB64 is required"
            ))),
        }
    }
}

impl PaneSetLabelParams {
    /// `set` と `unset` の少なくとも一方が必要。
    pub fn require_change(&self, method: &str) -> RpcResult<()> {
        if self.set.is_none() && self.unset.is_none() {
            return Err(RpcFailure::invalid_params(format!(
                "invalid params for {method}: set or unset is required"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn err(method: &str, v: Value) -> String {
        parse::<WindowCreateParams>(method, Some(&obj(v)))
            .unwrap_err()
            .message
    }

    #[test]
    fn names_must_be_non_empty() {
        assert!(
            err("window.create", json!({"workspace": "", "name": "x"}))
                .starts_with("invalid params for window.create:")
        );
        assert!(
            parse::<WindowCreateParams>(
                "window.create",
                Some(&obj(json!({"workspace": "a", "name": "x", "extra": 1})))
            )
            .is_ok()
        );
    }

    #[test]
    fn ids_must_match_the_ulid_pattern() {
        let ok = format!("p_{}", "0".repeat(26));
        assert!(parse::<PaneRefParams>("pane.info", Some(&obj(json!({"paneId": ok})))).is_ok());
        for bad in [
            "p_",
            "w_00000000000000000000000000",
            "p_80000000000000000000000000",
            "p_0000000000000000000000000I",
        ] {
            assert!(
                parse::<PaneRefParams>("pane.info", Some(&obj(json!({"paneId": bad})))).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn dimensions_are_positive_integers() {
        let p = |v: Value| parse::<PaneResizeParams>("pane.resize", Some(&obj(v)));
        let id = format!("p_{}", "0".repeat(26));
        assert_eq!(
            p(json!({"paneId": id, "cols": 80, "rows": 24.0}))
                .unwrap()
                .rows
                .get(),
            24
        );
        for bad in [json!(0), json!(-1), json!(1.5), json!("80")] {
            assert!(p(json!({"paneId": id, "cols": bad, "rows": 24})).is_err());
        }
    }

    #[test]
    fn since_accepts_zero_but_not_negatives() {
        let p = |v: Value| parse::<EventsSubscribeParams>("events.subscribe", Some(&obj(v)));
        assert_eq!(p(json!({"since": 0})).unwrap().since.map(Seq::get), Some(0));
        assert!(p(json!({"since": -1})).is_err());
        assert!(p(json!({"epoch": "nope"})).is_err());
    }

    #[test]
    fn list_filter_rejects_unknown_conditions() {
        let p = |v: Value| parse::<PaneListParams>("pane.list", Some(&obj(v)));
        assert!(p(json!({"filter": {"state": "running"}})).is_ok());
        assert!(p(json!({"filter": {"colour": "red"}})).is_err());
        assert!(p(json!({"filter": {"state": "paused"}})).is_err());
    }

    #[test]
    fn missing_params_are_an_empty_object() {
        assert!(parse::<Empty>("server.info", None).is_ok());
        assert!(parse::<WorkspaceNameParams>("workspace.close", None).is_err());
    }
}
