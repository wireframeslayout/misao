//! 1 行ぶんの入力の分類と、リクエストのディスパッチ。

use std::sync::Arc;

use misao_protocol::{ErrorCode, Incoming, METHODS, Request, Response, parse_incoming};
use serde_json::Value;

use crate::connection::ConnHandle;
use crate::core::Core;
use crate::rpc_error::{RpcFailure, RpcResult};

/// 応答を送った後に実行する処理（購読ストリームの開始など。応答より先に通知を出さないため）。
pub type AfterReply = Box<dyn FnOnce() + Send>;

/// 1 リクエストの間だけ持つ文脈。
pub struct RequestContext {
    pub conn: Arc<ConnHandle>,
    after: Vec<AfterReply>,
}

impl RequestContext {
    pub fn after_reply(&mut self, f: impl FnOnce() + Send + 'static) {
        self.after.push(Box::new(f));
    }
}

/// 完了まで時間のかかるメソッド（子プロセスの終了待ち）。同じ接続の後続のリクエストを止めないよう、別タスクで処理する。
/// それ以外は到着順に処理する（`pane.write` の順序を保つため）。
fn is_slow(method: &str) -> bool {
    matches!(method, "pane.close" | "workspace.close" | "window.close")
}

/// 1 行の入力を処理する。クライアントからの通知・応答は無視する。
pub async fn handle_line(core: &Arc<Core>, conn: &Arc<ConnHandle>, line: &str) {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => {
            conn.send_response(&Response::parse_error(e.to_string()));
            return;
        }
    };
    match parse_incoming(&value) {
        Incoming::Ignored => {}
        Incoming::Invalid { id } => conn.send_response(&Response::invalid_request(id)),
        Incoming::Request(req) if is_slow(&req.method) => {
            tokio::spawn(run_request(Arc::clone(core), Arc::clone(conn), req));
        }
        Incoming::Request(req) => run_request(Arc::clone(core), Arc::clone(conn), req).await,
    }
}

async fn run_request(core: Arc<Core>, conn: Arc<ConnHandle>, req: Request) {
    let mut ctx = RequestContext {
        conn: Arc::clone(&conn),
        after: Vec::new(),
    };
    match dispatch(&core, &req, &mut ctx).await {
        Ok(result) => conn.send_response(&Response::success(req.id, result)),
        Err(failure) => {
            if failure.code == ErrorCode::INTERNAL {
                core.log
                    .error(&format!("internal error in {}: {}", req.method, failure.message));
            }
            conn.send_response(&Response::error(Some(req.id), failure.code, failure.message));
            return;
        }
    }
    for f in ctx.after {
        f();
    }
}

async fn dispatch(core: &Arc<Core>, req: &Request, ctx: &mut RequestContext) -> RpcResult<Value> {
    let method = req.method.as_str();
    if !METHODS.contains(&method) {
        return Err(RpcFailure::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("unknown method: {method}"),
        ));
    }
    core.call(method, req.params.as_ref(), ctx).await
}
