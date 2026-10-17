//! `pane.attach` の接続側の状態（Node 版の `attach.ts` の listener / queue）。
//!
//! attach の応答を返すまでは、届いた出力を溜めるだけ（`Pending`）。応答を送った後に `go_live` が
//! 再生（replay）→ 溜めた出力（head より新しいものだけ）の順に送り、以後はライブで送る（`Live`）。
//! 出力は pane の `on_output`（順序づけの lock の中）から届く。送る先の送信キューが上限を超えた接続は `ConnHandle` が切る。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::connection::ConnHandle;

/// 応答前に溜める出力の上限（送信キューの上限と同じ）。
const MAX_PENDING_BYTES: usize = crate::connection::MAX_WRITABLE_BYTES;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `pane.output` 通知の 1 行（NDJSON）。`replay` は snapshot の再生だけに付く。
pub fn output_line(pane_id: &str, seq: u64, ts: &str, data: &[u8], replay: Option<&str>) -> Vec<u8> {
    let mut params = json!({"seq": seq, "ts": ts, "paneId": pane_id, "dataB64": STANDARD.encode(data)});
    if let (Some(replay), Some(map)) = (replay, params.as_object_mut()) {
        map.insert("replay".to_string(), Value::String(replay.to_string()));
    }
    let mut line = serde_json::to_vec(&json!({"jsonrpc": "2.0", "method": "pane.output", "params": params}))
        .unwrap_or_default();
    line.push(b'\n');
    line
}

struct Queued {
    seq: u64,
    ts: String,
    data: Vec<u8>,
}

enum State {
    /// 応答前。再生の行と、届いた出力を溜める。
    Pending {
        replay: Vec<Vec<u8>>,
        head: u64,
        queue: Vec<Queued>,
        /// `queue` に溜めたバイト数。応答が遅れても無制限には溜めない。
        queued_bytes: usize,
    },
    Live,
    /// 溜めすぎて接続を切った。以後は何も届けない。
    Dropped,
}

pub struct Attachment {
    pub id: u64,
    pub client_id: String,
    pane_id: String,
    conn: Weak<ConnHandle>,
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Attachment {
    /// `head` は再生が覆う最後の seq。再生の行は `set_replay` で後から渡す。
    pub fn new(pane_id: &str, client_id: &str, conn: &Arc<ConnHandle>, head: u64) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            client_id: client_id.to_string(),
            pane_id: pane_id.to_string(),
            conn: Arc::downgrade(conn),
            state: Mutex::new(State::Pending {
                replay: Vec::new(),
                head,
                queue: Vec::new(),
                queued_bytes: 0,
            }),
        })
    }

    pub fn conn(&self) -> Option<Arc<ConnHandle>> {
        self.conn.upgrade()
    }

    /// 応答の後に最初に送る行（raw の再生または snapshot）。`go_live` の前に呼ぶ。
    pub fn set_replay(&self, lines: Vec<Vec<u8>>) {
        if let State::Pending { replay, .. } = &mut *lock(&self.state) {
            *replay = lines;
        }
    }

    /// 1 回の出力を届ける。`line` は同じ pane の全 attach で共通の通知（ライブで送る形）。
    pub fn deliver(&self, seq: u64, ts: &str, data: &[u8], line: &[u8]) {
        let mut state = lock(&self.state);
        match &mut *state {
            State::Pending {
                queue, queued_bytes, ..
            } => {
                if *queued_bytes + data.len() > MAX_PENDING_BYTES {
                    // 応答前にこれ以上は溜められない。attach を諦めて接続を切る（接続の後始末が pane から外す）。
                    *state = State::Dropped;
                    drop(state);
                    if let Some(conn) = self.conn.upgrade() {
                        conn.destroy();
                    }
                    return;
                }
                *queued_bytes += data.len();
                queue.push(Queued {
                    seq,
                    ts: ts.to_string(),
                    data: data.to_vec(),
                });
            }
            State::Dropped => {}
            State::Live => {
                if let Some(conn) = self.conn.upgrade() {
                    conn.send_bytes(line.to_vec());
                }
            }
        }
    }

    /// 応答を送った後に呼ぶ。再生 → 溜めた出力 → ライブの順に切り替える。
    pub fn go_live(&self) {
        let mut state = lock(&self.state);
        if !matches!(*state, State::Pending { .. }) {
            return;
        }
        let State::Pending {
            replay, head, queue, ..
        } = std::mem::replace(&mut *state, State::Live)
        else {
            return;
        };
        let Some(conn) = self.conn.upgrade() else {
            return;
        };
        for line in replay {
            conn.send_bytes(line);
        }
        for q in queue.into_iter().filter(|q| q.seq > head) {
            conn.send_bytes(output_line(&self.pane_id, q.seq, &q.ts, &q.data, None));
        }
    }
}
