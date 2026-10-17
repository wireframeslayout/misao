//! 実行中のペイン 1 つぶん: 擬似端末の子プロセス、入力、サイズ、終了の検知（Node 版の `Pane`）。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use portable_pty::MasterPty;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::child_env::build_child_env;
use crate::event_log::EventLog;
use crate::ids::new_pane_id;
use crate::log::now_iso;
use crate::pty::{self, ExitInfo, PtyProcess, SpawnRequest};
use crate::size_arbiter::{SizeArbiter, SizeDecision};

/// close の SIGHUP から SIGKILL までの猶予（Node 版の `KILL_GRACE_MS`）。
const KILL_GRACE: Duration = Duration::from_secs(3);
/// 子の終了後、残りの出力を読み切るのを待つ上限（孫が pty を握り続けて EOF にならない場合の打ち切り）。
const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// close で、リーダーの終了後にグループの残りが消えるのを確かめる間隔と、SIGKILL 後に待つ上限。
const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(20);
const GROUP_KILL_WAIT: Duration = Duration::from_secs(2);
const DEFAULT_COLS: u32 = 80;
const DEFAULT_ROWS: u32 = 24;
const READ_CHUNK: usize = 16 * 1024;

pub struct PaneOpenOptions<'a> {
    pub cmd: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    /// spawn 時に子の環境へ足すだけで、Pane は保持しない。env と同じキーならこちらが勝つ。
    pub ephemeral_env: BTreeMap<String, String>,
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    pub socket_path: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Exited,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
        }
    }
}

struct Inner {
    state: RunState,
    exit: Option<ExitInfo>,
    last_output_at: Option<String>,
    cols: u32,
    rows: u32,
    sizes: SizeArbiter,
}

/// `start` が取り出す、スレッドへ渡すもの。
struct Handoff {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

pub struct Pane {
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub pid: u32,
    inner: Mutex<Inner>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    handoff: Mutex<Option<Handoff>>,
    /// 入力を pty へ書くスレッドへの送り口（pty の書き込みは詰まりうるので、RPC のスレッドでは書かない）。
    input: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    /// 子のプロセス（リーダー）が終了し、`pane.exited` を出し終えたら true。
    exited_tx: watch::Sender<bool>,
    /// 終了後の後片付け（close のときのプロセスグループの掃除）まで済んだら true。
    settled_tx: watch::Sender<bool>,
    /// close が始まった時刻（SIGHUP を送った時刻）。2 回目以降の close は何もせず終了を待つ。
    closing: Mutex<Option<Instant>>,
    released: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pane {
    /// 子プロセスを起動する。入出力のスレッドは `start` まで動かさない。
    pub fn spawn(opts: PaneOpenOptions<'_>) -> Result<Arc<Self>, String> {
        let cwd = match opts.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir()
                .map_err(|e| format!("cannot determine the working directory: {e}"))?
                .to_string_lossy()
                .into_owned(),
        };
        let (cols, rows) = (
            opts.cols.unwrap_or(DEFAULT_COLS),
            opts.rows.unwrap_or(DEFAULT_ROWS),
        );
        let id = new_pane_id();
        let mut overrides = opts.env;
        overrides.extend(opts.ephemeral_env);
        let env = build_child_env(std::env::vars_os(), &id, opts.socket_path, &overrides);
        let PtyProcess {
            pid,
            master,
            reader,
            writer,
            child,
        } = pty::spawn(&SpawnRequest {
            cmd: &opts.cmd,
            cwd: &cwd,
            env,
            cols,
            rows,
        })?;
        Ok(Arc::new(Self {
            id,
            cmd: opts.cmd,
            cwd,
            pid,
            inner: Mutex::new(Inner {
                state: RunState::Running,
                exit: None,
                last_output_at: None,
                cols,
                rows,
                sizes: SizeArbiter::default(),
            }),
            master: Mutex::new(master),
            handoff: Mutex::new(Some(Handoff {
                reader,
                writer,
                child,
            })),
            input: Mutex::new(None),
            exited_tx: watch::Sender::new(false),
            settled_tx: watch::Sender::new(false),
            closing: Mutex::new(None),
            released: AtomicBool::new(false),
        }))
    }

    /// 出力の読み取り・入力の書き込み・終了の待機を始める。`pane.opened` を出した後に呼ぶ
    /// （`pane.exited` が `pane.opened` より先に出ないように）。2 回目以降は何もしない。
    pub fn start(self: &Arc<Self>, events: Arc<EventLog>) {
        let Some(Handoff {
            mut reader,
            mut writer,
            child,
        }) = lock(&self.handoff).take()
        else {
            return;
        };
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        *lock(&self.input) = Some(input_tx);
        std::thread::spawn(move || {
            while let Ok(data) = input_rx.recv() {
                if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
            // 子が終わった後に writer を閉じても pty に何も書かない（portable-pty は drop で EOF を書く）ので、
            // ここでは何もせず drop する。入力を送る経路は、終了後は pane.write が拒否する。
        });
        let (drained_tx, drained_rx) = mpsc::channel::<()>();
        let pane = Arc::clone(self);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => pane.on_output(&buf[..n]),
                }
            }
            let _ = drained_tx.send(());
        });
        let pane = Arc::clone(self);
        std::thread::spawn(move || {
            let exit = pty::wait_exit(child, pane.pid);
            // 残りの出力を読み切ってから終了を報告する（終了直前の出力を取りこぼさない）。
            if let Err(RecvTimeoutError::Timeout) = drained_rx.recv_timeout(READER_DRAIN_TIMEOUT) {
                // 孫が pty を握っていて EOF が来ない。ここまでに読めた分で打ち切る。
            }
            pane.on_exit(exit, &events);
            pane.settle();
        });
    }

    fn on_output(&self, _data: &[u8]) {
        lock(&self.inner).last_output_at = Some(now_iso());
    }

    fn on_exit(&self, exit: ExitInfo, events: &EventLog) {
        {
            let mut inner = lock(&self.inner);
            inner.state = RunState::Exited;
            inner.exit = Some(exit);
        }
        events.emit(
            "pane.exited",
            json!({"exitCode": exit.exit_code, "signal": exit.signal}),
            Some(&self.id),
        );
        // 稼働判定は後続の段。終了だけは確定的なので、exit 段の結果を出す。
        events.emit(
            "pane.state",
            json!({"state": "exited", "decidedBy": "exit", "prev": "unknown"}),
            Some(&self.id),
        );
        self.exited_tx.send_replace(true);
    }

    /// リーダーの終了後の後片付け。close のときは、グループに残ったプロセスを猶予の後に SIGKILL で掃除する。
    fn settle(&self) {
        let deadline = lock(&self.closing).map(|started| started + KILL_GRACE);
        if let Some(deadline) = deadline {
            while pty::tree_alive(self.pid) {
                if Instant::now() >= deadline {
                    pty::kill_tree(self.pid);
                    let give_up = Instant::now() + GROUP_KILL_WAIT;
                    while pty::tree_alive(self.pid) && Instant::now() < give_up {
                        std::thread::sleep(GROUP_POLL_INTERVAL);
                    }
                    break;
                }
                std::thread::sleep(GROUP_POLL_INTERVAL);
            }
        }
        self.settled_tx.send_replace(true);
    }

    pub fn state(&self) -> RunState {
        lock(&self.inner).state
    }

    pub fn size(&self) -> (u32, u32) {
        let inner = lock(&self.inner);
        (inner.cols, inner.rows)
    }

    /// 入力を pty へ送る（書き込みは専用スレッド）。
    pub fn write(&self, data: Vec<u8>) {
        if let Some(tx) = lock(&self.input).as_ref() {
            let _ = tx.send(data);
        }
    }

    /// client のサイズを記録し、そのクライアントをサイズ所有者にして pty へ適用する。
    pub fn resize(&self, cols: u32, rows: u32, client: Option<&str>) -> Result<(), String> {
        let mut inner = lock(&self.inner);
        let decision = inner.sizes.record(client, cols, rows);
        self.apply(&mut inner, &decision)
    }

    /// client が所有者でなく、サイズを記録済みなら、そのサイズへ戻す。変更したら true。
    pub fn claim_size(&self, client: &str) -> Result<bool, String> {
        let mut inner = lock(&self.inner);
        let Some(decision) = inner.sizes.claim(client) else {
            return Ok(false);
        };
        self.apply(&mut inner, &decision)?;
        Ok(true)
    }

    fn apply(&self, inner: &mut Inner, decision: &SizeDecision) -> Result<(), String> {
        inner.cols = decision.cols;
        inner.rows = decision.rows;
        if inner.state == RunState::Running {
            lock(&self.master)
                .resize(pty::pty_size(decision.cols, decision.rows))
                .map_err(|e| format!("cannot resize the pty: {e:#}"))?;
        }
        Ok(())
    }

    /// `pane.info` の本体（所属の workspace / window とラベルは Daemon が渡す。Pane は親もラベルも知らない）。
    pub fn info(&self, workspace: &str, window: &Value, labels: &Value) -> Value {
        let inner = lock(&self.inner);
        let exit = inner.exit;
        json!({
            "paneId": self.id,
            "pid": self.pid,
            "cmd": self.cmd,
            "cwd": self.cwd,
            "workspace": workspace,
            "window": window,
            "labels": labels,
            "processState": inner.state.as_str(),
            "exitCode": exit.and_then(|e| e.exit_code),
            "signal": exit.and_then(|e| e.signal),
            "agentState": if inner.state == RunState::Exited { "exited" } else { "unknown" },
            "decidedBy": if inner.state == RunState::Exited { "exit" } else { "none" },
            "title": "",
            "lastOutputAt": inner.last_output_at,
            "cols": inner.cols,
            "rows": inner.rows,
            "clients": [],
            "sizeOwner": inner.sizes.owner(),
        })
    }

    /// SIGHUP → 3 秒 → SIGKILL（プロセスグループ全体）。リーダーの終了と、グループに残ったプロセスの
    /// 掃除まで済んだら戻る。並行して呼ばれても SIGHUP と kill タイマーは 1 回だけ。
    pub async fn close(self: &Arc<Self>) {
        let first = {
            let mut closing = lock(&self.closing);
            let first = closing.is_none() && self.state() == RunState::Running;
            if first {
                *closing = Some(Instant::now());
            }
            first
        };
        if first {
            pty::hangup_tree(self.pid);
            let pane = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(KILL_GRACE).await;
                if pane.state() == RunState::Running {
                    pty::kill_tree(pane.pid);
                }
            });
        }
        let mut settled = self.settled_tx.subscribe();
        let _ = settled.wait_for(|done| *done).await;
    }

    /// 閉じた Pane への参照を購読に残さないための印。
    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
    }

    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
}
