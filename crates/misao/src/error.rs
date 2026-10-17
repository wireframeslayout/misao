use serde_json::json;

use crate::config::ConfigError;
use crate::socket_path::PathError;

/// 利用者へ伝える失敗の種類（`--json` のエラー出力の `code`）。Node 版の `CliErrorKind` の部分集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Usage,
    Config,
    Runtime,
}

impl ErrorKind {
    fn name(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Config => "config",
            Self::Runtime => "runtime",
        }
    }
}

/// コマンドが利用者へ伝える失敗。終了コードは usage だけ 2、それ以外は 1。
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn runtime(error: &impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::Runtime, error.to_string())
    }

    pub fn exit_code(&self) -> u8 {
        if self.kind == ErrorKind::Usage { 2 } else { 1 }
    }

    /// stderr に出す 1 行。`--json` のときは `{"error":{"code","message"}}`。
    pub fn render(&self, is_json: bool) -> String {
        if is_json {
            json!({ "error": { "code": self.kind.name(), "message": self.message } }).to_string()
        } else {
            format!("[misao] {}", self.message)
        }
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
