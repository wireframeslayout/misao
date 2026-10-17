//! 実行中のペイン 1 つぶん: 擬似端末の子プロセス、入力、サイズ、終了の検知（Node 版の `Pane`）。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use crate::limits::{INPUT_QUEUE_BYTES, LINE_ENTRY_OVERHEAD};
use crate::log::now_iso;
use crate::pty::{self, ExitInfo, PtyProcess, SpawnRequest};
use crate::ring::SeqRing;
use crate::size_arbiter::{SizeArbiter, SizeDecision};
use crate::stream::{Progress, Wakeup};

#[path = "pane_lines.rs"]
mod lines;

pub use lines::LinesSource;

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
    /// 子が終了すると外す（送り口を閉じて入力のスレッドと writer を終わらせる）。
    input: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    /// 入力の待ち行列に溜まっているバイト数（上限は `INPUT_QUEUE_BYTES`）。
    input_queued: Arc<AtomicUsize>,
    /// 子のプロセス（リーダー）が終了し、`pane.exited` を出し終えたら true。
    /// 行リングの head。新しい行が入るたびに進む。
    lines_tx: watch::Sender<u64>,
    exited_tx: watch::Sender<bool>,
    /// リーダーの終了時に、プロセスグループに他のプロセスが残っていたか（Unix のみ）。
    survivors_at_exit: AtomicBool,
    /// 保留していたリーダーを回収した（以後は pid が再利用されうる）。
    reaped: AtomicBool,
    /// close が始まった時刻（SIGHUP を送った時刻）。2 回目以降の close は何もせず終了を待つ。
    closing: Mutex<Option<Instant>>,
    released: AtomicBool,
    /// 子のプロセスが終了した（残りの出力は待たずに読み切る）。
    exit_seen: AtomicBool,
    /// 行ストリームの購読者の進み具合（送り手が遅い購読者を待つため）。
    line_subscribers: Mutex<Vec<Weak<Progress>>>,
    lines_ring_bytes: usize,
    /// pty を読むスレッドが購読者を待つ間の起床。購読者の進み・購読の終了・close・子の終了・release で起こす。
    wakeup: Arc<Wakeup>,
}

impl Drop for Pane {
    /// close されないまま捨てられても、終了済みのリーダーをゾンビで残さない。
    fn drop(&mut self) {
        self.reap_leader();
    }
}

impl Inner {
    /// 行リングの 1 エントリは、文字数（UTF-16 の長さ）+ 固定のオーバーヘッドで数える（Node 版と同じ）。
    fn push_line(&mut self, text: String, ts: String) {
        let size = text.chars().map(char::len_utf16).sum::<usize>() + LINE_ENTRY_OVERHEAD;
        self.lines.push(text, size, ts);
    }
}

/// プロセスの停止はブロックしうる（Windows の `taskkill` など）ので、非同期のタスクを止めないよう別のスレッドで行う。
async fn blocking(f: impl FnOnce() + Send + 'static) {
    let _ = tokio::task::spawn_blocking(f).await;
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
            input_queued: Arc::new(AtomicUsize::new(0)),
            lines_tx: watch::Sender::new(0),
            exited_tx: watch::Sender::new(false),
            survivors_at_exit: AtomicBool::new(false),
            reaped: AtomicBool::new(false),
            closing: Mutex::new(None),
            released: AtomicBool::new(false),
            exit_seen: AtomicBool::new(false),
            line_subscribers: Mutex::new(Vec::new()),
            lines_ring_bytes: opts.lines_ring_bytes,
            wakeup: Arc::new(Wakeup::default()),
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
        let queued = Arc::clone(&self.input_queued);
        std::thread::spawn(move || {
            while let Ok(data) = input_rx.recv() {
                let written = writer.write_all(&data).and_then(|()| writer.flush());
                queued.fetch_sub(data.len(), Ordering::AcqRel);
                if written.is_err() {
                    break;
                }
            }
            // 送り口が閉じた（子が終了した）か、書けなくなった。writer はここで drop される
            // （portable-pty は drop のときに改行と EOF を pty へ書くが、子が居ない pty なので害はない）。
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
            // リーダーを回収しない環境（`pty::LEADER_HELD`）では、close まで pid が再利用されない。
            // そうでない環境では、終了時に残りがいたかを記録し、いなければ close はグループに触れない。
            pane.survivors_at_exit
                .store(pty::tree_alive(pane.pid), Ordering::Release);
            pane.exit_seen.store(true, Ordering::Release);
            pane.wakeup.notify();
            // 残りの出力を読み切ってから終了を報告する（終了直前の出力を取りこぼさない）。
            if let Err(RecvTimeoutError::Timeout) = drained_rx.recv_timeout(READER_DRAIN_TIMEOUT) {
                // 孫が pty を握っていて EOF が来ない。ここまでに読めた分で打ち切る。
            }
            pane.on_exit(exit, &events);
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

    fn on_exit(&self, exit: ExitInfo, events: &EventLog) {
        // 入力の送り口を閉じ、書き込みのスレッドと writer（master の複製）を終わらせる。
        lock(&self.input).take();
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

    pub fn state(&self) -> RunState {
        lock(&self.inner).state
    }

    pub fn size(&self) -> (u64, u64) {
        let inner = lock(&self.inner);
        (inner.cols, inner.rows)
    }

    /// `len` バイトの入力を待ち行列に受けられるか。溜まっているぶん（バイト数）を Err で返す。
    /// 待ち行列が空なら、上限より大きい 1 回の入力も受ける。
    pub fn check_input_room(&self, len: usize) -> Result<(), usize> {
        let queued = self.input_queued.load(Ordering::Acquire);
        if queued > 0 && queued.saturating_add(len) > INPUT_QUEUE_BYTES {
            return Err(queued);
        }
        Ok(())
    }

    /// 入力を pty へ送る（書き込みは専用スレッドで、ここでは待たない）。待ち行列が一杯なら溜まっているバイト数を Err で返す。
    /// 終了した pane では何もしない（呼び出し側が先に拒否する）。
    pub fn write(&self, data: Vec<u8>) -> Result<(), usize> {
        let input = lock(&self.input);
        let Some(tx) = input.as_ref() else {
            return Ok(());
        };
        self.check_input_room(data.len())?;
        self.input_queued.fetch_add(data.len(), Ordering::AcqRel);
        let _ = tx.send(data);
        Ok(())
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
    /// 掃除まで済んだら戻る。リーダーが既に（自然に）終了していても、グループに残ったプロセスがいれば同じ手順で止める。
    /// 並行して呼ばれても SIGHUP は 1 回だけで、猶予は最初の呼び出しから数える。
    pub async fn close(self: &Arc<Self>) {
        let pid = self.pid;
        let (started, first) = {
            let mut closing = lock(&self.closing);
            match *closing {
                Some(started) => (started, false),
                None => {
                    let now = Instant::now();
                    *closing = Some(now);
                    (now, true)
                }
            }
        };
        self.wakeup.notify();
        let deadline = started + KILL_GRACE;
        let was_running = self.state() == RunState::Running;
        // リーダーを回収していないなら pid は自分のもの。そうでなければ、終了時に残りがいたときだけ触れる。
        let owns_group = || {
            was_running
                || (pty::LEADER_HELD && !self.reaped.load(Ordering::Acquire))
                || self.survivors_at_exit.load(Ordering::Acquire)
        };
        if first && owns_group() && (was_running || pty::tree_alive(pid)) {
            blocking(move || pty::hangup_tree(pid)).await;
        }
        let mut exited = self.exited_tx.subscribe();
        if tokio::time::timeout_at(deadline.into(), exited.wait_for(|e| *e))
            .await
            .is_err()
        {
            blocking(move || pty::kill_tree(pid)).await;
            let _ = exited.wait_for(|e| *e).await;
        }
        if owns_group() {
            self.sweep_group(deadline).await;
        }
        self.reap_leader();
    }

    /// 回収を保留していたリーダーを回収する（グループの片付けの後。一度だけ）。
    fn reap_leader(&self) {
        if self.state() == RunState::Exited && !self.reaped.swap(true, Ordering::AcqRel) {
            pty::reap(self.pid);
        }
    }

    /// グループに残ったプロセスが消えるのを待ち、猶予が過ぎても残っていれば SIGKILL する。
    async fn sweep_group(&self, deadline: Instant) {
        let pid = self.pid;
        while pty::tree_alive(pid) {
            if Instant::now() >= deadline {
                blocking(move || pty::kill_tree(pid)).await;
                let give_up = Instant::now() + GROUP_KILL_WAIT;
                while pty::tree_alive(pid) && Instant::now() < give_up {
                    tokio::time::sleep(GROUP_POLL_INTERVAL).await;
                }
                return;
            }
            tokio::time::sleep(GROUP_POLL_INTERVAL).await;
        }
    }

    /// 閉じた Pane への参照を購読に残さないための印。待っている購読を起こして止める。
    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.wakeup.notify();
        self.lines_tx.send_modify(|_| {});
    }

    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
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

    /// リーダーが自然に終了し、SIGHUP を無視する子が残った pane。
    fn pane_with_a_survivor(dir: &Path) -> (Arc<Pane>, PathBuf) {
        let (pid_file, ready) = (dir.join("survivor.pid"), dir.join("ready"));
        let script = format!(
            "(trap '' HUP; touch {ready}; exec sleep 300) & pid=$!; while [ ! -e {ready} ]; do sleep 0.05; done; echo $pid > {pid}",
            ready = ready.display(),
            pid = pid_file.display()
        );
        let socket = PathBuf::from("/tmp/unused.sock");
        let pane = Pane::spawn(PaneOpenOptions {
            cmd: vec!["sh".into(), "-c".into(), script],
            cwd: None,
            env: BTreeMap::new(),
            ephemeral_env: BTreeMap::new(),
            cols: None,
            rows: None,
            socket_path: &socket,
            raw_ring_bytes: 1024,
            lines_ring_bytes: 1024,
        })
        .unwrap();
        pane.start(Arc::new(EventLog::new(10)));
        (pane, pid_file)
    }

    fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_input_channel_is_closed_when_the_child_exits() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (pane, pid_file) = pane_with_a_survivor(tmp.path());
        wait_for("the pane to exit", || pane.state() == RunState::Exited);
        assert!(std::fs::read_to_string(pid_file).is_ok());
        wait_for("the input channel to be released", || lock(&pane.input).is_none());
        crate::pty::kill_tree(pane.pid);
    }

    #[test]
    fn the_input_queue_refuses_input_beyond_its_limit_but_accepts_one_into_an_empty_queue() {
        let socket = PathBuf::from("/tmp/unused.sock");
        let pane = Pane::spawn(PaneOpenOptions {
            cmd: vec!["sleep".into(), "30".into()],
            cwd: None,
            env: BTreeMap::new(),
            ephemeral_env: BTreeMap::new(),
            cols: None,
            rows: None,
            socket_path: &socket,
            raw_ring_bytes: 1024,
            lines_ring_bytes: 1024,
        })
        .unwrap();
        // 待ち行列が空なら、上限より大きい入力も受ける。
        assert!(pane.check_input_room(INPUT_QUEUE_BYTES * 2).is_ok());
        // 書き込みのスレッドが詰まっている状況（start せず、送り口だけがある）で溜める。
        let (tx, _rx) = mpsc::channel();
        *lock(&pane.input) = Some(tx);
        assert!(pane.write(vec![0; INPUT_QUEUE_BYTES - 10]).is_ok());
        assert!(pane.write(vec![0; 10]).is_ok(), "exactly at the limit");
        assert_eq!(pane.write(vec![0; 1]), Err(INPUT_QUEUE_BYTES));
        assert_eq!(pane.check_input_room(1), Err(INPUT_QUEUE_BYTES));
        crate::pty::kill_tree(pane.pid);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_after_a_natural_exit_releases_the_threads_and_the_master() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (pane, _) = pane_with_a_survivor(tmp.path());
        wait_for("the pane to exit", || pane.state() == RunState::Exited);
        // 孫が pty を握っているので、reader はまだ EOF になっていない（Pane への参照を持ち続ける）。
        let weak = Arc::downgrade(&pane);
        pane.close().await;
        assert!(!crate::pty::tree_alive(pane.pid));
        drop(pane);
        // reader / 待機のスレッドが終われば、最後の参照（= master と writer）が解放される。
        wait_for("the pane to be freed", || weak.upgrade().is_none());
    }
}
