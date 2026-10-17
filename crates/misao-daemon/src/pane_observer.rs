//! `PaneView` を複数のスレッドから使えるようにし、期限の来た判定を実行して、起きた出来事をイベントとして出す。
//!
//! 画面モデルと稼働判定の状態は 1 つの lock で守り、イベントはその lock の中で出す（出来事の起きた順とイベントの
//! 順序を一致させる）。判定を駆動するスレッドは `Pane` を知らず、このオブジェクトだけを持つので、`Pane` が捨てられても
//! 残らない（`close` で止める）。

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::event_log::EventLog;
use crate::pane_view::{PaneView, ScreenSnapshot, ViewEvent};

struct Shared {
    view: PaneView,
    closed: bool,
}

pub struct PaneObserver {
    shared: Mutex<Shared>,
    /// 期限が変わったとき（出力・終了・close）に、判定のスレッドを起こす。
    wake: Condvar,
    /// `PaneView` の時刻の原点（pane の起動）。
    epoch: Instant,
    pane_id: String,
    events: Arc<EventLog>,
}

/// `pane.info` の稼働判定とタイトル。
pub struct ObservedInfo {
    pub agent_state: &'static str,
    pub decided_by: String,
    pub title: String,
}

impl PaneObserver {
    pub fn new(pane_id: String, view: PaneView, events: Arc<EventLog>) -> Arc<Self> {
        Arc::new(Self {
            shared: Mutex::new(Shared { view, closed: false }),
            wake: Condvar::new(),
            epoch: Instant::now(),
            pane_id,
            events,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn emit(&self, events: Vec<ViewEvent>) {
        for event in events {
            match event {
                ViewEvent::Title(title) => self.events.emit("pane.title", json!({"title": title}), Some(&self.pane_id)),
                ViewEvent::State(change) => self.events.emit(
                    "pane.state",
                    json!({"state": change.state.as_str(), "decidedBy": change.decided_by, "prev": change.prev.as_str()}),
                    Some(&self.pane_id),
                ),
            }
        }
    }

    pub fn output(&self, data: &[u8]) {
        let mut shared = self.lock();
        let events = shared.view.output(data, self.now());
        self.emit(events);
        self.wake.notify_all();
    }

    pub fn input(&self) {
        let now = self.now();
        self.lock().view.input(now);
    }

    pub fn resize(&self, cols: u64, rows: u64) {
        let now = self.now();
        self.lock().view.resize(cols, rows, now);
    }

    /// 子の終了。`emit_exit` で `pane.exited` を出した直後に、exit 段の状態を確定する（その間に他の出来事は挟まらない）。
    pub fn exited(&self, emit_exit: impl FnOnce()) {
        let mut shared = self.lock();
        emit_exit();
        let events = shared.view.exited();
        self.emit(events);
        shared.closed = true;
        self.wake.notify_all();
    }

    pub fn snapshot(&self) -> ScreenSnapshot {
        self.lock().view.snapshot()
    }

    pub fn info(&self) -> ObservedInfo {
        let shared = self.lock();
        let (state, decided_by) = shared.view.agent_state();
        ObservedInfo {
            agent_state: state.as_str(),
            decided_by: decided_by.to_string(),
            title: shared.view.title().to_string(),
        }
    }

    /// 判定のスレッドを止める。
    pub fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }

    /// 判定を駆動する。期限が来るまで眠り、来たら `advance` する。`close` で戻る。
    pub fn run(self: &Arc<Self>) {
        let mut shared = self.lock();
        while !shared.closed {
            let now = self.now();
            let events = shared.view.advance(now);
            self.emit(events);
            shared = match shared.view.next_deadline() {
                Some(at) => {
                    let wait = Duration::from_millis(at.saturating_sub(self.now()).max(1));
                    self.wake
                        .wait_timeout(shared, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self.wake.wait(shared).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}
