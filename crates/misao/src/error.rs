use serde_json::{Map, Value, json};

use crate::config::ConfigError;
use crate::socket_path::PathError;

/// 利用者へ伝える失敗の種類（`--json` のエラー出力の `code`）。Node 版の `CliErrorKind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Usage,
    NotFound,
    Ambiguous,
    Aborted,
    DaemonUnreachable,
    Rpc,
    Config,
    Runtime,
}

impl ErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::NotFound => "not_found",
            Self::Ambiguous => "ambiguous",
            Self::Aborted => "aborted",
            Self::DaemonUnreachable => "daemon_unreachable",
            Self::Rpc => "rpc",
            Self::Config => "config",
            Self::Runtime => "runtime",
        }
    }
}

/// あいまいな対象指定の候補。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub pane_id: String,
    pub name: String,
}

/// コマンドが利用者へ伝える失敗。終了コードは usage だけ 2、それ以外は 1。
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
    pub candidates: Option<Vec<Candidate>>,
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            candidates: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    pub fn runtime(error: &impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::Runtime, error.to_string())
    }

    pub fn with_candidates(mut self, candidates: Vec<Candidate>) -> Self {
        self.candidates = Some(candidates);
        self
    }

    pub fn exit_code(&self) -> u8 {
        if self.kind == ErrorKind::Usage { 2 } else { 1 }
    }

    /// stderr に出す文字列（行は改行で区切る）。`--json` のときは `{"error":{code,message,candidates?}}`。
    pub fn render(&self, is_json: bool) -> String {
        if is_json {
            let mut body = Map::new();
            body.insert("code".into(), json!(self.kind.name()));
            body.insert("message".into(), json!(self.message));
            if let Some(candidates) = &self.candidates {
                let list: Vec<Value> = candidates
                    .iter()
                    .map(|c| json!({"paneId": c.pane_id, "name": c.name}))
                    .collect();
                body.insert("candidates".into(), Value::Array(list));
            }
            return json!({ "error": body }).to_string();
        }
        let mut lines = vec![format!("[misao] {}", self.message)];
        for c in self.candidates.iter().flatten() {
            lines.push(format!("  {}  {}", c.pane_id, c.name));
        }
        lines.join("\n")
    }
}

impl From<ConfigError> for CliError {
    fn from(e: ConfigError) -> Self {
        Self::new(ErrorKind::Config, e.to_string())
    }
}

impl From<PathError> for CliError {
    fn from(e: PathError) -> Self {
        Self::new(ErrorKind::Config, format!("invalid path setting: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_usage_exits_with_2() {
        assert_eq!(CliError::usage("x").exit_code(), 2);
        assert_eq!(CliError::new(ErrorKind::Rpc, "x").exit_code(), 1);
    }

    #[test]
    fn renders_human_and_json_with_candidates() {
        let err = CliError::new(ErrorKind::Ambiguous, "many").with_candidates(vec![Candidate {
            pane_id: "p_1".into(),
            name: "a".into(),
        }]);
        assert_eq!(err.render(false), "[misao] many\n  p_1  a");
        let json: Value = serde_json::from_str(&err.render(true)).unwrap_or_default();
        assert_eq!(json["error"]["code"], "ambiguous");
        assert_eq!(json["error"]["candidates"][0]["paneId"], "p_1");
    }
}
