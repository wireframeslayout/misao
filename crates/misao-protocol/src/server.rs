use serde::{Deserialize, Serialize};

/// `server.info` の結果。項目は `packages/protocol/src/methods/server.ts` のスキーマと一致させる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub protocol_version: String,
    /// デーモンのビルド版。version を返さない古いデーモンもあるので省略可。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub pid: u32,
    /// 起動ごとに変わる ULID。
    pub epoch: String,
    pub uptime_sec: u64,
    pub pane_count: u64,
    pub event_head: u64,
}
