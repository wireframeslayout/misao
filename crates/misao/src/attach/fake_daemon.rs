//! テスト用の偽デーモン。NDJSON JSON-RPC の応答を決め打ちで返し、受けた呼び出しを記録する
//! （Node 版のテストが実デーモンを起動する代わり。attach の入出力まわりだけを確かめるのに足りる最小限）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::model::PaneInfo;

pub const EPOCH: &str = "01M3AAAAAAAAAAAAAAAAAAAAAA";

#[derive(Default)]
struct State {
    panes: Vec<Value>,
    calls: Vec<(String, Value)>,
    /// 接続ごとの送り口（デーモンから流す通知用）。
    conns: Vec<UnboundedSender<Option<String>>>,
    /// pane.attach を拒否する。
    reject_attach: bool,
}

pub struct FakeDaemon {
    pub socket: PathBuf,
    state: Arc<Mutex<State>>,
    _dir: TempDir,
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn pane_json(pane_id: &str, edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = json!({
        "paneId": pane_id,
        "pid": 100,
        "cmd": ["sh"],
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
    value
}

impl FakeDaemon {
    pub async fn start(panes: Vec<Value>) -> Self {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let socket = dir.path().join("fake.sock");
        let listener = UnixListener::bind(&socket).unwrap_or_else(|e| panic!("{e}"));
        let state = Arc::new(Mutex::new(State {
            panes,
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&shared)));
            }
        });
        Self {
            socket,
            state,
            _dir: dir,
        }
    }

    pub fn pane(&self, pane_id: &str) -> PaneInfo {
        let found = lock(&self.state)
            .panes
            .iter()
            .find(|p| p["paneId"] == pane_id)
            .cloned()
            .unwrap_or_else(|| panic!("no pane {pane_id}"));
        PaneInfo::from_value(found).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn set_pane(&self, pane_id: &str, edit: impl FnOnce(&mut Value)) {
        let mut state = lock(&self.state);
        if let Some(p) = state.panes.iter_mut().find(|p| p["paneId"] == pane_id) {
            edit(p);
        }
    }

    pub fn remove_pane(&self, pane_id: &str) {
        lock(&self.state).panes.retain(|p| p["paneId"] != pane_id);
    }

    pub fn reject_attach(&self) {
        lock(&self.state).reject_attach = true;
    }

    /// 記録した呼び出し（メソッド名と params）。
    pub fn calls(&self, method: &str) -> Vec<Value> {
        lock(&self.state)
            .calls
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }

    fn broadcast(&self, line: String) {
        for tx in &lock(&self.state).conns {
            let _ = tx.send(Some(line.clone()));
        }
    }

    /// 接続している全クライアントへ pane.output を流す。
    pub fn output(&self, pane_id: &str, data: &[u8]) {
        let msg = json!({"jsonrpc": "2.0", "method": "pane.output", "params": {"seq": 1, "ts": "2026-01-01T00:00:00.000Z", "paneId": pane_id, "dataB64": STANDARD.encode(data)}});
        self.broadcast(msg.to_string());
    }

    /// デーモンのイベントを流す（events.subscribe の購読者向け）。
    pub fn event(&self, seq: u64, kind: &str, pane_id: &str, data: &Value) {
        let msg = json!({"jsonrpc": "2.0", "method": "event", "params": {"seq": seq, "ts": "2026-01-01T00:00:00.000Z", "type": kind, "paneId": pane_id, "data": data}});
        self.broadcast(msg.to_string());
    }

    /// 全接続を切る（デーモンが落ちた状況）。
    pub fn drop_connections(&self) {
        for tx in lock(&self.state).conns.drain(..) {
            let _ = tx.send(None);
        }
    }
}

async fn serve(stream: tokio::net::UnixStream, state: Arc<Mutex<State>>) {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = unbounded_channel::<Option<String>>();
    lock(&state).conns.push(tx.clone());
    let writer = tokio::spawn(async move {
        while let Some(Some(line)) = rx.recv().await {
            if write.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (id, method) = (
            request["id"].clone(),
            request["method"].as_str().unwrap_or_default().to_string(),
        );
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let response = handle(&state, &method, &params);
        let message = match response {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, text)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": text}})
            }
        };
        let _ = tx.send(Some(message.to_string()));
    }
    writer.abort();
}

fn handle(state: &Mutex<State>, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    let mut state = lock(state);
    state.calls.push((method.to_string(), params.clone()));
    let find = |state: &State| {
        state
            .panes
            .iter()
            .find(|p| p["paneId"] == params["paneId"])
            .cloned()
            .ok_or((1001, format!("pane not found: {}", params["paneId"])))
    };
    match method {
        "server.info" => Ok(
            json!({"protocolVersion": "0.3.0", "pid": 1, "epoch": EPOCH, "uptimeSec": 1, "paneCount": state.panes.len(), "eventHead": 0}),
        ),
        "events.subscribe" => Ok(json!({"gap": false, "head": 0, "epoch": EPOCH})),
        "pane.list" => Ok(Value::Array(state.panes.clone())),
        "pane.info" => find(&state),
        "pane.attach" => {
            if state.reject_attach {
                return Err((1001, "pane not found".to_string()));
            }
            find(&state)?;
            Ok(json!({"head": 0, "oldest": 1, "truncated": false}))
        }
        "pane.detach" | "pane.write" | "pane.resize" => Ok(json!({"ok": true})),
        "pane.open" => {
            let id = format!("p_01M3NEW{:0>19}", state.panes.len());
            let pane = pane_json(&id, |v| {
                v["cmd"] = params["cmd"].clone();
                v["labels"] = params["labels"].clone();
                v["cwd"] = params["cwd"].clone();
            });
            state.panes.push(pane);
            Ok(json!({"paneId": id}))
        }
        other => Err((-32601, format!("unknown method: {other}"))),
    }
}
