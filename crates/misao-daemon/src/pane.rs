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
use crate::foreground::ForegroundQuery;
use crate::ids::new_pane_id;
use crate::limits::{INPUT_QUEUE_BYTES, LINE_ENTRY_OVERHEAD};
use crate::log::now_iso;
use crate::pane_observer::PaneObserver;
use crate::pane_view::{PaneView, ScreenSnapshot};
use crate::profile::{Profile, select_profile};
use crate::pty::{self, ExitInfo, GroupState, PtyProcess, SpawnRequest};
use crate::ring::SeqRing;
use crate::size_arbiter::{SizeArbiter, SizeDecision};
use crate::stream::{Progress, Wakeup};

#[path = "pane_close.rs"]
mod close;
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
    /// 画面モデルのスクロールバック行数。
    pub scrollback: usize,
    /// 稼働判定に使いうるプロファイル。pane の起動コマンドと前面のプログラムで選ぶ（無ければ title / bytes 段だけで判定する）。
    pub profiles: Vec<Arc<Profile>>,
    /// `pane.title` / `pane.state` / `pane.exited` を出す先。
    pub events: Arc<EventLog>,
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
    /// 出力の取り込み（リングへの格納と端末の解析）と resize を、受け取った順に画面へ反映するための順序づけ。
    /// 解析は `inner` の外で行うので、これが無いと、格納済みの出力の解析の前に resize が入り、古い出力が新しいサイズで解析される。
    /// ロックの順序: order -> inner -> observer。
    order: Mutex<()>,
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
    /// close の片付けを始めた（最初の `close` だけが独立したタスクとして起動する）。
    close_started: AtomicBool,
    /// close の片付けが完了したら true。
    closed_tx: watch::Sender<bool>,
    released: AtomicBool,
    /// 子のプロセスが終了した（残りの出力は待たずに読み切る）。
    exit_seen: AtomicBool,
    /// 画面・タイトル・稼働判定（`pane.screen` / `pane.info` / `pane.title` / `pane.state`）。
    observer: Arc<PaneObserver>,
    events: Arc<EventLog>,
    /// 行ストリームの購読者の進み具合（送り手が遅い購読者を待つため）。
    line_subscribers: Mutex<Vec<Weak<Progress>>>,
    lines_ring_bytes: usize,
    /// pty を読むスレッドが購読者を待つ間の起床。購読者の進み・購読の終了・close・子の終了・release で起こす。
    wakeup: Arc<Wakeup>,
}

impl Drop for Pane {
    /// close されないまま捨てられても、終了済みのリーダーをゾンビで残さない。
    fn drop(&mut self) {
        self.observer.close();
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
        let observer = PaneObserver::new(
            id.clone(),
            PaneView::new(
                cols,
                rows,
                opts.scrollback,
                select_profile(&opts.profiles, &opts.cmd),
            ),
            Arc::clone(&opts.events),
            opts.profiles,
            opts.cmd.clone(),
        );
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
            order: Mutex::new(()),
            input_queued: Arc::new(AtomicUsize::new(0)),
            lines_tx: watch::Sender::new(0),
            exited_tx: watch::Sender::new(false),
            survivors_at_exit: AtomicBool::new(false),
            reaped: AtomicBool::new(false),
            closing: Mutex::new(None),
            close_started: AtomicBool::new(false),
            closed_tx: watch::Sender::new(false),
            released: AtomicBool::new(false),
            exit_seen: AtomicBool::new(false),
            line_subscribers: Mutex::new(Vec::new()),
            lines_ring_bytes: opts.lines_ring_bytes,
            wakeup: Arc::new(Wakeup::default()),
            observer,
            events: opts.events,
        }))
    }

    /// 出力の読み取り・入力の書き込み・終了の待機を始める。`pane.opened` を出した後に呼ぶ
    /// （`pane.exited` が `pane.opened` より先に出ないように）。2 回目以降は何もしない。
    pub fn start(self: &Arc<Self>) {
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
        let observer = Arc::clone(&self.observer);
        std::thread::spawn(move || observer.run());
        let (drained_tx, drained_rx) = mpsc::channel::<()>();
        // reader はブロッキングの read の間 Pane を強参照しない。孫が pty を握って EOF が来なくても、
        // 持ち主が Pane を捨てれば `Drop`（リーダーの回収）が走る。
        let pane = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_CHUNK];
            loop {
                match pane.upgrade() {
                    Some(pane) => pane.wait_for_subscribers(),
                    None => break,
                }
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => match pane.upgrade() {
                        Some(pane) => pane.on_output(&buf[..n]),
                        None => break,
                    },
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
                .store(pty::group_state(pane.pid) != GroupState::Empty, Ordering::Release);
            pane.exit_seen.store(true, Ordering::Release);
            pane.wakeup.notify();
            // 残りの出力を読み切ってから終了を報告する（終了直前の出力を取りこぼさない）。
            if let Err(RecvTimeoutError::Timeout) = drained_rx.recv_timeout(READER_DRAIN_TIMEOUT) {
                // 孫が pty を握っていて EOF が来ない。ここまでに読めた分で打ち切る。
            }
            pane.on_exit(exit);
        });
    }

    fn on_output(&self, data: &[u8]) {
        let _order = lock(&self.order);
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
        // 端末の解析は、リングの更新の lock を離してから行う（解析が長くても、リングを読む側を待たせない）。
        // 出力のスレッドは 1 つなので、順序は保たれる。
        self.observer.output(data);
    }

    fn on_exit(&self, exit: ExitInfo) {
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
        // pane.exited の直後に pane.state(exited)（exit 段）を確定する。
        self.observer.exited(|| {
            self.events.emit(
                "pane.exited",
                json!({"exitCode": exit.exit_code, "signal": exit.signal}),
                Some(&self.id),
            );
        });
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
        self.observer.input();
        self.input_queued.fetch_add(data.len(), Ordering::AcqRel);
        let _ = tx.send(data);
        Ok(())
    }

    /// client のサイズを記録し、そのクライアントをサイズ所有者にして pty へ適用する。
    pub fn resize(&self, cols: u64, rows: u64, client: Option<&str>) -> Result<(), String> {
        let _order = lock(&self.order);
        let mut inner = lock(&self.inner);
        let decision = inner.sizes.record(client, cols, rows);
        self.apply(&mut inner, &decision)
    }

    /// client が所有者でなく、サイズを記録済みなら、そのサイズへ戻す。変更したら true。
    pub fn claim_size(&self, client: &str) -> Result<bool, String> {
        let _order = lock(&self.order);
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
        self.observer.resize(decision.cols, decision.rows);
        if inner.state == RunState::Running {
            lock(&self.master)
                .resize(pty::pty_size(decision.cols, decision.rows))
                .map_err(|e| format!("cannot resize the pty: {e:#}"))?;
        }
        Ok(())
    }

    /// `pane.info` の本体（所属の workspace / window とラベルは Daemon が渡す。Pane は親もラベルも知らない）。
    /// 前面のプログラムで profile を選び直す必要があるか（走行中で、起動コマンドだけでは決まらない pane）。
    pub fn needs_foreground_probe(&self) -> bool {
        self.state() == RunState::Running && self.observer.needs_foreground_probe()
    }

    /// 調べた前面のプログラムで、profile を選び直す。
    pub fn apply_foreground(&self, foreground: Option<&str>) {
        self.observer.apply_foreground(foreground);
    }

    /// 前面プロセスの問い合わせ（走行中の pane だけ）。
    pub fn foreground_query(&self) -> Option<ForegroundQuery> {
        (self.state() == RunState::Running).then(|| ForegroundQuery {
            pid: self.pid,
            pty_pgid: self.pty_foreground_pgid(),
        })
    }

    /// `pane.info` の本体（所属の workspace / window とラベル、前面プロセス名は呼び出し側が渡す。Pane は親もラベルも知らない）。
    pub fn info(&self, workspace: &str, window: &Value, labels: &Value, fg_command: Option<String>) -> Value {
        let observed = self.observer.info();
        let inner = lock(&self.inner);
        let exit = inner.exit;
        let mut info = json!({
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
            "agentState": observed.agent_state,
            "decidedBy": observed.decided_by,
            "title": observed.title,
            "lastOutputAt": inner.last_output_at,
            "cols": inner.cols,
            "rows": inner.rows,
            "clients": [],
            "sizeOwner": inner.sizes.owner(),
        });
        if let (Some(command), Some(map)) = (fg_command, info.as_object_mut()) {
            map.insert("fgCommand".to_string(), json!(command));
        }
        info
    }

    /// pty の前面プロセスグループ（Linux 以外の Unix の `fgCommand` が使う。Linux は `/proc` から引くので pty を触らない）。
    fn pty_foreground_pgid(&self) -> Option<i32> {
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            lock(&self.master).process_group_leader()
        }
        #[cfg(not(all(unix, not(target_os = "linux"))))]
        {
            None
        }
    }

    /// `pane.screen` の本体。
    pub fn screen(&self) -> ScreenSnapshot {
        self.observer.snapshot()
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
#[path = "pane_tests.rs"]
mod tests;
