use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::ErrorCode;

/// JSON-RPC の id。数値または文字列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(Number),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: RequestId,
    pub method: String,
    pub params: Option<Map<String, Value>>,
}

/// 受信した 1 行ぶんの値の分類（デーモンの `onMessage` と同じ振り分け）。
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 通知・応答など、サーバーが処理しないもの。黙って捨てる。
    Ignored,
    /// リクエストとして不正。`id` は取れたときだけ入る。
    Invalid {
        id: Option<RequestId>,
    },
    Request(Request),
}

fn request_id(value: &Value) -> Option<RequestId> {
    match value {
        Value::Number(n) => Some(RequestId::Number(n.clone())),
        Value::String(s) => Some(RequestId::String(s.clone())),
        _ => None,
    }
}

/// 1 行の JSON 値を分類する。オブジェクトでなければ Invalid（id 無し）。
/// `method` と `id` の両方を持たないものは Ignored。
pub fn parse_incoming(value: &Value) -> Incoming {
    let Value::Object(obj) = value else {
        return Incoming::Invalid { id: None };
    };
    let (Some(id_value), Some(method_value)) = (obj.get("id"), obj.get("method")) else {
        return Incoming::Ignored;
    };
    let id = request_id(id_value);
    let params = match obj.get("params") {
        None => Some(None),
        Some(Value::Object(map)) => Some(Some(map.clone())),
        Some(_) => None,
    };
    match (obj.get("jsonrpc"), id, method_value, params) {
        (Some(Value::String(v)), Some(id), Value::String(method), Some(params)) if v == "2.0" => {
            Incoming::Request(Request {
                id,
                method: method.clone(),
                params,
            })
        }
        (_, id, ..) => Incoming::Invalid { id },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: i64,
    pub message: String,
}

/// サーバーから送る応答。結果とエラーのどちらか一方だけを持つ。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Response {
    Success {
        jsonrpc: &'static str,
        id: RequestId,
        result: Value,
    },
    Error {
        jsonrpc: &'static str,
        /// リクエストを解釈できなかったときは null。
        id: Option<RequestId>,
        error: ErrorBody,
    },
}

impl Response {
    pub fn success(id: RequestId, result: Value) -> Self {
        Self::Success {
            jsonrpc: "2.0",
            id,
            result,
        }
    }

    pub fn error(id: Option<RequestId>, code: i64, message: impl Into<String>) -> Self {
        Self::Error {
            jsonrpc: "2.0",
            id,
            error: ErrorBody {
                code,
                message: message.into(),
            },
        }
    }

    pub fn invalid_request(id: Option<RequestId>) -> Self {
        Self::error(id, ErrorCode::INVALID_REQUEST, "invalid request")
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::error(None, ErrorCode::PARSE, message)
    }
}
