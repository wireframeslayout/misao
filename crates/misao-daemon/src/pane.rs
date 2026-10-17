//! 実行中のペイン 1 つぶん: 擬似端末の子プロセス、入力、サイズ、終了の検知（Node 版の `Pane`）。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use portable_pty::MasterPty;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::ansi::AnsiLineAssembler;
use crate::child_env::build_child_env;
use crate::event_log::EventLog;
use crate::ids::new_pane_id;
use crate::limits::LINE_ENTRY_OVERHEAD;
use crate::log::now_iso;
use crate::pty::{self, ExitInfo, PtyProcess, SpawnRequest};
use crate::ring::SeqRing;
use crate::size_arbiter::{SizeArbiter, SizeDecision};
use crate::stream::{Next, Progress, StreamSource};

/// close の SIGHUP から SIGKILL までの猶予（Node 版の `KILL_GRACE_MS`）。
const KILL_GRACE: Duration = Duration::from_secs(3);
/// 子の終了後、残りの出力を読み切るのを待つ上限（孫が pty を握り続けて EOF にならない場合の打ち切り）。
const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// close で、リーダーの終了後にグループの残りが消えるのを確かめる間隔と、SIGKILL 後に待つ上限。
const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(20);
const GROUP_KILL_WAIT: Duration = Duration::from_secs(2);
const DEFAULT_COLS: u64 = 80;
const DEFAULT_ROWS: u64 = 24;
const READ_CHUNK: usize = 16 * 1024;
/// 行ストリームの購読者の遅れがリング容量のこの割合を超えたら、追いつくまで pty を読まない（子の書き込みを止める）。
/// 超えるまでは読み続け、リングが遅い購読者を吸収する。
const LAG_LIMIT_DIVISOR: u64 = 2;
/// 購読者を待っている間に、遅れを見直す間隔。
const THROTTLE_POLL: Duration = Duration::from_millis(1);

pub struct PaneOpenOptions<'a> {
    pub cmd: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    /// spawn 時に子の環境へ足すだけで、Pane は保持しない。env と同じキーならこちらが勝つ。
    pub ephemeral_env: BTreeMap<String, String>,
    pub cols: Option<u64>,
    pub rows: Option<u64>,
    pub socket_path: &'a Path,
    /// 生出力リングの上限バイト数。
    pub raw_ring_bytes: usize,
    /// 行ストリームリングの上限バイト数。
    pub lines_ring_bytes: usize,
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
    cols: u64,
    rows: u64,
    sizes: SizeArbiter,
    /// 生の出力（pty から読んだままのバイト列）。attach の再生に使う（後続の段）。
    raw: SeqRing<Vec<u8>>,
    /// ANSI を除いた行。`pane.subscribe_lines` が読む。
    lines: SeqRing<String>,
    assembler: AnsiLineAssembler,
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
    /// 行リングの head。新しい行が入るたびに進む。
    lines_tx: watch::Sender<u64>,
    exited_tx: watch::Sender<bool>,
    /// 終了後の後片付け（close のときのプロセスグループの掃除）まで済んだら true。
    settled_tx: watch::Sender<bool>,
    /// close が始まった時刻（SIGHUP を送った時刻）。2 回目以降の close は何もせず終了を待つ。
    closing: Mutex<Option<Instant>>,
    released: AtomicBool,
    /// 子のプロセスが終了した（残りの出力は待たずに読み切る）。
    exit_seen: AtomicBool,
    /// 行ストリームの購読者の進み具合（送り手が遅い購読者を待つため）。
    line_subscribers: Mutex<Vec<Weak<Progress>>>,
    lines_ring_bytes: usize,
}

impl Inner {
    /// 行リングの 1 エントリは、文字数（UTF-16 の長さ）+ 固定のオーバーヘッドで数える（Node 版と同じ）。
    fn push_line(&mut self, text: String, ts: String) {
        let size = text.chars().map(char::len_utf16).sum::<usize>() + LINE_ENTRY_OVERHEAD;
        self.lines.push(text, size, ts);
    }
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
                raw: SeqRing::new(opts.raw_ring_bytes),
                lines: SeqRing::new(opts.lines_ring_bytes),
                assembler: AnsiLineAssembler::default(),
            }),
            master: Mutex::new(master),
            handoff: Mutex::new(Some(Handoff {
                reader,
                writer,
                child,
            })),
            input: Mutex::new(None),
            lines_tx: watch::Sender::new(0),
            exited_tx: watch::Sender::new(false),
            settled_tx: watch::Sender::new(false),
            closing: Mutex::new(None),
            released: AtomicBool::new(false),
            exit_seen: AtomicBool::new(false),
            line_subscribers: Mutex::new(Vec::new()),
            lines_ring_bytes: opts.lines_ring_bytes,
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
                pane.wait_for_subscribers();
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
            pane.exit_seen.store(true, Ordering::Release);
            // 残りの出力を読み切ってから終了を報告する（終了直前の出力を取りこぼさない）。
            if let Err(RecvTimeoutError::Timeout) = drained_rx.recv_timeout(READER_DRAIN_TIMEOUT) {
                // 孫が pty を握っていて EOF が来ない。ここまでに読めた分で打ち切る。
            }
            pane.on_exit(exit, &events);
            pane.settle();
        });
    }

    fn on_output(&self, data: &[u8]) {
        let ts = now_iso();
        let new_head = {
            let mut inner = lock(&self.inner);
            inner.last_output_at = Some(ts.clone());
            inner.raw.push(data.to_vec(), data.len(), ts.clone());
            let lines = inner.assembler.push(data);
            let any = !lines.is_empty();
            for text in lines {
                inner.push_line(text, ts.clone());
            }
            any.then(|| inner.lines.head())
        };
        if let Some(head) = new_head {
            self.lines_tx.send_replace(head);
        }
    }

    /// 行ストリームの購読者が、リングに追い越されそうなほど遅れている間は pty を読まない。
    /// 読まなければ子の書き込みが詰まるので、動いている（カーソルが進んでいる）購読者は取りこぼさない。
    /// 止まっている購読者（読んでいない）は待たず、リングに追い越されたら切断する。
    /// 閉じている途中・子の終了後は待たない（出力を読み切って終わらせる）。
    fn wait_for_subscribers(&self) {
        while self.is_lagging() {
            std::thread::sleep(THROTTLE_POLL);
        }
    }

    fn is_lagging(&self) -> bool {
        if self.exit_seen.load(Ordering::Acquire) || self.is_released() || lock(&self.closing).is_some() {
            return false;
        }
        let live: Vec<Arc<Progress>> = {
            let mut subscribers = lock(&self.line_subscribers);
            subscribers.retain(|p| p.strong_count() > 0);
            subscribers.iter().filter_map(Weak::upgrade).collect()
        };
        if live.is_empty() {
            return false;
        }
        let limit = self.lines_ring_bytes as u64 / LAG_LIMIT_DIVISOR;
        let inner = lock(&self.inner);
        let head = inner.lines.cumulative_head();
        live.iter().any(|p| {
            !p.is_stalled()
                && inner
                    .lines
                    .cumulative_at(p.cursor())
                    .is_some_and(|delivered| head - delivered > limit)
        })
    }

    fn on_exit(&self, exit: ExitInfo, events: &EventLog) {
        let new_head = {
            let mut inner = lock(&self.inner);
            // 改行なしで終わった最終行も行ストリームに流す
            let rest = inner.assembler.pending().to_string();
            let any = !rest.is_empty();
            if any {
                inner.push_line(rest, now_iso());
            }
            inner.state = RunState::Exited;
            inner.exit = Some(exit);
            any.then(|| inner.lines.head())
        };
        if let Some(head) = new_head {
            self.lines_tx.send_replace(head);
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

    pub fn size(&self) -> (u64, u64) {
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
    pub fn resize(&self, cols: u64, rows: u64, client: Option<&str>) -> Result<(), String> {
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

    /// 閉じた Pane への参照を購読に残さないための印。待っている購読を起こして止める。
    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.lines_tx.send_modify(|_| {});
    }

    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
}

/// `pane.subscribe_lines` の購読元（行リング）。
pub struct LinesSource(pub Arc<Pane>);

impl StreamSource for LinesSource {
    fn bounds(&self) -> (u64, u64) {
        let inner = lock(&self.0.inner);
        (inner.lines.head(), inner.lines.oldest())
    }

    fn next(&self, cursor: u64, writable: bool) -> Next {
        if self.0.is_released() {
            return Next::Released;
        }
        let inner = lock(&self.0.inner);
        if cursor + 1 < inner.lines.oldest() {
            return Next::Overtaken;
        }
        if cursor >= inner.lines.head() {
            return Next::Empty;
        }
        if !writable {
            return Next::Blocked;
        }
        match inner.lines.entry(cursor + 1) {
            Some(entry) => Next::Entry {
                seq: entry.seq,
                notification: json!({
                    "jsonrpc": "2.0",
                    "method": "pane.line",
                    "params": {"seq": entry.seq, "ts": entry.ts, "paneId": self.0.id, "text": entry.item},
                }),
            },
            None => Next::Overtaken,
        }
    }

    fn wake(&self) -> watch::Receiver<u64> {
        self.0.lines_tx.subscribe()
    }

    fn track(&self, progress: &Arc<Progress>) {
        lock(&self.0.line_subscribers).push(Arc::downgrade(progress));
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    impl Pane {
        /// 生出力リングの `(oldest, head)`。
        fn raw_bounds(&self) -> (u64, u64) {
            let inner = lock(&self.inner);
            (inner.raw.oldest(), inner.raw.head())
        }

        fn raw_bytes(&self) -> usize {
            let inner = lock(&self.inner);
            (inner.raw.oldest()..=inner.raw.head())
                .filter_map(|seq| inner.raw.entry(seq).map(|e| e.item.len()))
                .sum()
        }
    }

    #[test]
    fn the_raw_ring_keeps_only_the_newest_bytes_up_to_its_capacity() {
        let socket = PathBuf::from("/tmp/unused.sock");
        let pane = Pane::spawn(PaneOpenOptions {
            cmd: vec![
                "sh".into(),
                "-c".into(),
                "head -c 20000 /dev/zero | tr '\\0' x".into(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            ephemeral_env: BTreeMap::new(),
            cols: None,
            rows: None,
            socket_path: &socket,
            raw_ring_bytes: 4096,
            lines_ring_bytes: 1024,
        })
        .unwrap();
        pane.start(Arc::new(EventLog::new(10)));
        let deadline = Instant::now() + Duration::from_secs(10);
        while pane.state() != RunState::Exited {
            assert!(Instant::now() < deadline, "the pane did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        let (oldest, head) = pane.raw_bounds();
        assert!(
            oldest > 1 && head >= oldest,
            "old chunks were evicted: {oldest}..{head}"
        );
        // 直近のチャンクは、容量を超えていても 1 つは残る。それ以外は容量に収まる。
        assert!(pane.raw_bytes() <= 4096 + READ_CHUNK);
    }
}
