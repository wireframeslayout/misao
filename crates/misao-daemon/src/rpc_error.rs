use misao_protocol::ErrorCode;

/// ハンドラが返す、JSON-RPC のエラー応答になる失敗（Node 版の `RpcFailure`）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RpcFailure {
    pub code: i64,
    pub message: String,
}

pub type RpcResult<T> = Result<T, RpcFailure>;

impl RpcFailure {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INTERNAL, message)
    }
}
