//! 稼働判定のコア（Node 版の `StateTracker`）。exit > profile > title > bytes の順に見て、最初に意見を持った段で決める。
//! コアは blocked を出さない（blocked を出せるのはプロファイルだけ）。
//!
//! タイマーは持たない。時刻（pane の起動からのミリ秒）は全部引数で受け取り、期限は `next_deadline` で教える。
//! 実際に待つのは呼び出し側（pane の判定スレッド）で、期限が来たら `advance_to` を呼ぶ。そのため同じ入力列からは
//! 常に同じ判定列が出る（Node 版との差分テストはこれを使う）。

use std::sync::Arc;

use crate::byte_activity::{ActivityVerdict, ByteActivity};
use crate::profile::{Profile, ProfileScreen, ProfileVerdict};
use crate::title_activity::TitleActivity;

/// bytes 段の評価と、title 段の更新の古さの判定を行う間隔。
const TICK_MS: u64 = 1000;
/// プロファイルの画面判定は、出力が落ち着くまでこの時間待つ。
const PROFILE_DEBOUNCE_MS: u64 = 120;
/// 出力が続いても、最初の出力からこの時間以内には判定する。
const PROFILE_MAX_WAIT_MS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Unknown,
    Working,
    Blocked,
    Idle,
    Exited,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
            Self::Exited => "exited",
        }
    }
}

impl From<ActivityVerdict> for AgentState {
    fn from(verdict: ActivityVerdict) -> Self {
        match verdict {
            ActivityVerdict::Working => Self::Working,
            ActivityVerdict::Idle => Self::Idle,
        }
    }
}

impl From<ProfileVerdict> for AgentState {
    fn from(verdict: ProfileVerdict) -> Self {
        match verdict {
            ProfileVerdict::Working => Self::Working,
            ProfileVerdict::Blocked => Self::Blocked,
            ProfileVerdict::Idle => Self::Idle,
        }
    }
}

/// 状態または decidedBy が変わったこと（`pane.state` イベントの中身）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange {
    pub state: AgentState,
    pub decided_by: String,
    pub prev: AgentState,
}

/// 登録した順に実行する期限（同じ時刻なら登録が先のものから。Node の `setInterval` が実行のたびに登録し直されるのと同じ）。
#[derive(Debug, Clone, Copy)]
struct Deadline {
    at: u64,
    seq: u64,
}

pub struct StateTracker {
    state: AgentState,
    decided_by: String,
    is_exited: bool,
    /// 停止後は、遅れて届く画面更新の通知でタイマーを作り直さない。
    is_stopped: bool,
    profile: Option<Arc<Profile>>,
    profile_verdict: Option<ProfileVerdict>,
    bytes: ByteActivity,
    title: TitleActivity,
    byte_verdict: Option<ActivityVerdict>,
    tick: Option<Deadline>,
    profile_timer: Option<Deadline>,
    profile_wait_since: Option<u64>,
    next_seq: u64,
    changes: Vec<StateChange>,
}

impl StateTracker {
    pub fn new(now: u64, profile: Option<Arc<Profile>>) -> Self {
        let mut tracker = Self {
            state: AgentState::Unknown,
            decided_by: "none".to_string(),
            is_exited: false,
            is_stopped: false,
            profile,
            profile_verdict: None,
            bytes: ByteActivity::new(now),
            title: TitleActivity::default(),
            byte_verdict: None,
            tick: None,
            profile_timer: None,
            profile_wait_since: None,
            next_seq: 0,
            changes: Vec::new(),
        };
        tracker.tick = Some(tracker.deadline(now + TICK_MS));
        tracker
    }

    fn deadline(&mut self, at: u64) -> Deadline {
        self.next_seq += 1;
        Deadline {
            at,
            seq: self.next_seq,
        }
    }

    pub fn snapshot(&self) -> (AgentState, &str) {
        (self.state, &self.decided_by)
    }

    /// 溜まった状態の変化を取り出す（起きた順）。
    pub fn take_changes(&mut self) -> Vec<StateChange> {
        std::mem::take(&mut self.changes)
    }

    pub fn record_output(&mut self, bytes: usize, now: u64) {
        if !self.is_exited {
            self.bytes.record(bytes, now);
        }
    }

    /// 画面モデルが出力を解析し終えたときに呼ぶ。プロファイル判定はここから予約する。
    /// 出力の受信時に予約すると、解析前の古い画面を読んでしまう（最後のチャンクを取り逃す）。
    pub fn notify_screen_updated(&mut self, now: u64) {
        if self.is_stopped || self.profile.is_none() {
            return;
        }
        let since = *self.profile_wait_since.get_or_insert(now);
        let delay = PROFILE_DEBOUNCE_MS.min((since + PROFILE_MAX_WAIT_MS).saturating_sub(now));
        self.profile_timer = Some(self.deadline(now + delay));
    }

    /// この pane に使うプロファイルを替える（前面のプログラムが変わったとき）。同じものなら何もしない。
    /// 外すと、プロファイルの意見も判定の履歴も捨て、title / bytes 段だけで決め直す（どの段も意見を持たなければ `unknown` / `none`）。
    /// 付けると、いまの状態のまま、画面の判定を予約する（結果は 120ms 以内に出る）。title / bytes 段の履歴は残す。
    pub fn set_profile(&mut self, profile: Option<Arc<Profile>>, now: u64) {
        let same = match (&self.profile, &profile) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same || self.is_exited {
            return;
        }
        self.profile = profile;
        self.profile_verdict = None;
        self.profile_timer = None;
        self.profile_wait_since = None;
        if self.profile.is_some() {
            self.notify_screen_updated(now);
        } else {
            // 最終の判定を 1 度だけ出す（`unknown` を挟んだ偽の中間イベントを出さない）。
            let (state, decided_by) = self
                .opinion(now)
                .unwrap_or((AgentState::Unknown, "none".to_string()));
            self.decide(state, &decided_by);
        }
    }

    pub fn notify_input(&mut self, now: u64) {
        self.bytes.notify_input(now);
    }

    pub fn notify_resize(&mut self, now: u64) {
        self.bytes.notify_resize(now);
    }

    pub fn set_title(&mut self, title: &str, now: u64) {
        if self.is_exited {
            return;
        }
        self.title.set_title(title, now);
        self.evaluate(now);
    }

    /// 子プロセスの終了。終端の状態で、以後は何も判定しない。
    pub fn mark_exited(&mut self) {
        self.stop();
        self.is_exited = true;
        self.decide(AgentState::Exited, "exit");
    }

    /// タイマーを止める。状態は変えない。
    pub fn stop(&mut self) {
        self.is_stopped = true;
        self.tick = None;
        self.profile_timer = None;
        self.profile_wait_since = None;
    }

    /// 次に `advance_to` を呼ぶべき時刻。タイマーが止まっていれば None。
    pub fn next_deadline(&self) -> Option<u64> {
        self.due().map(|(_, deadline)| deadline.at)
    }

    fn due(&self) -> Option<(TimerKind, Deadline)> {
        let tick = self.tick.map(|d| (TimerKind::Tick, d));
        let profile = self.profile_timer.map(|d| (TimerKind::Profile, d));
        [tick, profile]
            .into_iter()
            .flatten()
            .min_by_key(|(_, d)| (d.at, d.seq))
    }

    /// 期限が来たタイマーを、期限の順に実行する（各タイマーは `now` の時点の出来事として扱い、次の tick は `now` から数える）。
    /// 呼び出し側は期限（`next_deadline`）になったら呼ぶ。`read_screen` は、プロファイルの判定が走るときだけ呼ばれる。
    pub fn advance_to(&mut self, now: u64, read_screen: &mut dyn FnMut() -> ProfileScreen) {
        while let Some((kind, _)) = self.due().filter(|(_, d)| d.at <= now) {
            match kind {
                TimerKind::Tick => {
                    self.tick = Some(self.deadline(now + TICK_MS));
                    self.byte_verdict = self.bytes.tick(now);
                    self.evaluate(now);
                }
                TimerKind::Profile => {
                    self.profile_timer = None;
                    self.profile_wait_since = None;
                    if let Some(profile) = &self.profile {
                        self.profile_verdict = profile.classify(&read_screen());
                    }
                    self.evaluate(now);
                }
            }
        }
    }

    /// 各段の意見を優先順に見て、最初に意見を持った段の判定（状態と decidedBy）。全段が意見なしなら None。
    fn opinion(&self, now: u64) -> Option<(AgentState, String)> {
        if let (Some(profile), Some(verdict)) = (&self.profile, self.profile_verdict) {
            return Some((verdict.into(), profile.name.clone()));
        }
        if let Some(verdict) = self.title.verdict(now) {
            return Some((verdict.into(), "title".to_string()));
        }
        self.byte_verdict
            .map(|verdict| (verdict.into(), "bytes".to_string()))
    }

    /// 意見を持った段があれば、それで決める。全段が意見なしなら現状維持。
    fn evaluate(&mut self, now: u64) {
        if self.is_exited {
            return;
        }
        if let Some((state, decided_by)) = self.opinion(now) {
            self.decide(state, &decided_by);
        }
    }

    fn decide(&mut self, state: AgentState, decided_by: &str) {
        if state == self.state && decided_by == self.decided_by {
            return;
        }
        let prev = self.state;
        self.state = state;
        self.decided_by = decided_by.to_string();
        self.changes.push(StateChange {
            state,
            decided_by: decided_by.to_string(),
            prev,
        });
    }
}

#[derive(Debug, Clone, Copy)]
enum TimerKind {
    Tick,
    Profile,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_screen() -> ProfileScreen {
        ProfileScreen {
            rows: Vec::new(),
            title: String::new(),
        }
    }

    /// 期限の来るたびにその時刻で `advance_to` を呼んで、`until` まで進める（実際の駆動と同じ）。
    fn run(tracker: &mut StateTracker, until: u64) {
        while let Some(deadline) = tracker.next_deadline().filter(|&d| d <= until) {
            tracker.advance_to(deadline, &mut no_screen);
        }
    }

    #[test]
    fn starts_unknown_and_goes_idle_after_five_quiet_seconds() {
        let mut tracker = StateTracker::new(0, None);
        assert_eq!(tracker.snapshot(), (AgentState::Unknown, "none"));
        run(&mut tracker, 4999);
        assert!(tracker.take_changes().is_empty());
        run(&mut tracker, 5000);
        assert_eq!(
            tracker.take_changes(),
            vec![StateChange {
                state: AgentState::Idle,
                decided_by: "bytes".into(),
                prev: AgentState::Unknown
            }]
        );
    }

    #[test]
    fn a_title_spinner_outranks_bytes_and_a_stale_one_gives_the_decision_back() {
        let mut tracker = StateTracker::new(0, None);
        tracker.set_title("⠋ build", 1200);
        assert_eq!(tracker.snapshot(), (AgentState::Working, "title"));
        run(&mut tracker, 5000);
        assert_eq!(tracker.snapshot(), (AgentState::Idle, "bytes"));
    }

    #[test]
    fn dropping_the_profile_emits_a_single_final_state_change() {
        let profile = crate::profile::builtin_profiles()
            .ok()
            .and_then(|p| p.into_iter().next());
        let mut tracker = StateTracker::new(0, profile);
        tracker.notify_screen_updated(0);
        tracker.advance_to(120, &mut || ProfileScreen {
            rows: vec!["Do you want to proceed?".into()],
            title: String::new(),
        });
        assert_eq!(tracker.snapshot(), (AgentState::Blocked, "claude"));
        // bytes 段は静かなまま 5 秒で idle の意見を持つ
        run(&mut tracker, 5000);
        tracker.take_changes();
        tracker.set_profile(None, 5100);
        assert_eq!(
            tracker.take_changes(),
            vec![StateChange {
                state: AgentState::Idle,
                decided_by: "bytes".into(),
                prev: AgentState::Blocked
            }]
        );
        // どの段も意見を持たなければ unknown / none へ、これも 1 件だけ
        let profile = crate::profile::builtin_profiles()
            .ok()
            .and_then(|p| p.into_iter().next());
        let mut tracker = StateTracker::new(0, profile);
        tracker.notify_screen_updated(0);
        tracker.advance_to(120, &mut || ProfileScreen {
            rows: vec!["Do you want to proceed?".into()],
            title: String::new(),
        });
        tracker.take_changes();
        tracker.set_profile(None, 200);
        assert_eq!(
            tracker.take_changes(),
            vec![StateChange {
                state: AgentState::Unknown,
                decided_by: "none".into(),
                prev: AgentState::Blocked
            }]
        );
    }

    #[test]
    fn exit_is_terminal() {
        let mut tracker = StateTracker::new(0, None);
        tracker.record_output(300, 100);
        run(&mut tracker, 1200);
        tracker.record_output(300, 1500);
        run(&mut tracker, 3000);
        tracker.take_changes();
        tracker.mark_exited();
        tracker.set_title("⠋ late", 3100);
        run(&mut tracker, 9000);
        assert_eq!(
            tracker.take_changes(),
            vec![StateChange {
                state: AgentState::Exited,
                decided_by: "exit".into(),
                prev: AgentState::Working
            }]
        );
        assert_eq!(tracker.next_deadline(), None);
    }

    #[test]
    fn the_profile_waits_for_a_quiet_moment_but_never_longer_than_300ms() {
        let profile = crate::profile::builtin_profiles()
            .ok()
            .and_then(|p| p.into_iter().next());
        let mut tracker = StateTracker::new(0, profile);
        run(&mut tracker, 1000);
        tracker.notify_screen_updated(1000);
        assert_eq!(tracker.next_deadline(), Some(1000 + 120));
        tracker.notify_screen_updated(1100);
        tracker.notify_screen_updated(1250);
        assert_eq!(tracker.next_deadline(), Some(1300));
        let mut reads = 0;
        tracker.advance_to(1300, &mut || {
            reads += 1;
            ProfileScreen {
                rows: vec!["✻ Pondering… (3s · esc to interrupt)".into()],
                title: String::new(),
            }
        });
        assert_eq!(reads, 1);
        assert_eq!(tracker.snapshot(), (AgentState::Working, "claude"));
        tracker.stop();
        tracker.notify_screen_updated(2000);
        assert_eq!(tracker.next_deadline(), None);
    }
}
