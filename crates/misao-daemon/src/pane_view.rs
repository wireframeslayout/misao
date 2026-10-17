//! ペインの「見え方」: 画面・タイトル・稼働判定をまとめたもの（Node 版の `Pane` のうち、`@xterm/headless` と
//! `StateTracker` を繋ぐ部分）。時刻（pane の起動からのミリ秒）は全部引数で受け取り、スレッドも I/O も持たない。
//! 同じ入力列からは常に同じ画面と判定列が出るので、Node 版との差分テストはこれをそのまま動かす。

use std::sync::Arc;

use crate::profile::{Profile, ProfileScreen};
use crate::screen::{Cursor, Screen};
use crate::state_tracker::{AgentState, StateChange, StateTracker};

/// 呼び出し側がイベントとして出す出来事（起きた順）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewEvent {
    /// `pane.title`。同じ値の再設定も 1 件ずつ出る（xterm の `onTitleChange` と同じ）。
    Title(String),
    /// `pane.state`。
    State(StateChange),
}

/// `pane.screen` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSnapshot {
    pub text: String,
    pub cursor: Cursor,
    pub alt_screen: bool,
    pub title: String,
    /// 出力と resize のたびに増える。値が変わっていなければ画面は変わっていない。
    pub activity: u64,
}

pub struct PaneView {
    screen: Screen,
    tracker: StateTracker,
    title: String,
    activity: u64,
}

impl PaneView {
    pub fn new(cols: u64, rows: u64, scrollback: usize, profile: Option<Arc<Profile>>) -> Self {
        Self {
            screen: Screen::new(cols, rows, scrollback),
            tracker: StateTracker::new(0, profile),
            title: String::new(),
            activity: 0,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn agent_state(&self) -> (AgentState, &str) {
        self.tracker.snapshot()
    }

    /// 子の出力。画面に入れ、タイトルの変化と稼働判定の変化を返す。
    pub fn output(&mut self, data: &[u8], now: u64) -> Vec<ViewEvent> {
        self.activity += 1;
        self.tracker.record_output(data.len(), now);
        let mut events = Vec::new();
        for title in self.screen.feed(data) {
            // 状態の変化は、それを起こしたタイトルの通知より先に出る（Node 版と同じ順序）。
            self.tracker.set_title(&title, now);
            self.drain_into(&mut events);
            events.push(ViewEvent::Title(title.clone()));
            self.title = title;
        }
        // プロファイルは解析済みの画面を読む必要があるので、解析の後に予約する。
        self.tracker.notify_screen_updated(now);
        events
    }

    /// 前面のプログラムが変わって、使うプロファイルが替わった。
    pub fn set_profile(&mut self, profile: Option<Arc<Profile>>, now: u64) -> Vec<ViewEvent> {
        self.tracker.set_profile(profile, now);
        let mut events = Vec::new();
        self.drain_into(&mut events);
        events
    }

    pub fn input(&mut self, now: u64) {
        self.tracker.notify_input(now);
    }

    pub fn resize(&mut self, cols: u64, rows: u64, now: u64) {
        self.activity += 1;
        self.tracker.notify_resize(now);
        self.screen.resize(cols, rows);
    }

    /// 子の終了。タイマーを止めて exit 段で確定する。
    pub fn exited(&mut self) -> Vec<ViewEvent> {
        self.tracker.mark_exited();
        let mut events = Vec::new();
        self.drain_into(&mut events);
        events
    }

    /// 期限の来た判定（bytes の tick、プロファイルの画面判定）を実行する。
    pub fn advance(&mut self, now: u64) -> Vec<ViewEvent> {
        let Self {
            screen,
            tracker,
            title,
            ..
        } = self;
        tracker.advance_to(now, &mut || ProfileScreen {
            rows: screen.rows(),
            title: title.clone(),
        });
        let mut events = Vec::new();
        self.drain_into(&mut events);
        events
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.tracker.next_deadline()
    }

    pub fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            text: self.screen.rows().join("\n"),
            cursor: self.screen.cursor(),
            alt_screen: self.screen.is_alt_screen(),
            title: self.title.clone(),
            activity: self.activity,
        }
    }

    fn drain_into(&mut self, events: &mut Vec<ViewEvent>) {
        events.extend(self.tracker.take_changes().into_iter().map(ViewEvent::State));
    }
}
