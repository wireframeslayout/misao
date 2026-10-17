//! 保存のタイミング（Node 版の `StatePersister`）。
//! - `save_now`: 定義・ラベルなど失うと困る変更。呼び出し側で同期して保存し、失敗は Err にする。
//! - `save_soon`: cols / rows のような、再起動後の表示用の付帯情報。1 秒遅らせて保存し、失敗はログだけ（RPC は落とさない）。
//!
//! ファイルへの書き込みは `write` の lock で直列にする。スナップショットはこの lock を取ってから作るので、
//! 古い状態が新しい状態を上書きすることはない（lock の順序: write -> 全体の状態）。fsync は全体の lock の外で行う。

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::log::Logger;
use crate::persistence::{PersistedState, save_persisted_state};

/// サイズ変更の保存を遅らせる時間。連続した resize を 1 回の保存にまとめる。
const SIZE_SAVE_DELAY: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Soon {
    /// 遅らせている保存がまだ済んでいない。期限とは分けて持つ（保存に失敗しても残す）。
    pending: bool,
    deadline: Option<Instant>,
    stopped: bool,
}

pub struct StatePersister {
    path: PathBuf,
    log: Logger,
    write: Mutex<()>,
    soon: Mutex<Soon>,
    timer: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StatePersister {
    pub fn new(path: &Path, log: Logger) -> Self {
        Self {
            path: path.to_path_buf(),
            log,
            write: Mutex::new(()),
            soon: Mutex::new(Soon::default()),
            timer: Condvar::new(),
        }
    }

    /// 書き込みの順序づけ。保存したい状態のスナップショットは、これを取ってから作る。
    pub fn write_guard(&self) -> MutexGuard<'_, ()> {
        lock(&self.write)
    }

    /// 壊れた状態（存在しない window を指す pane など）は書かずに Err にする。成功したら遅らせていた保存を取り消す
    /// （失敗したときは予約を残し、次の `save_soon` / `flush` で再試行する）。
    pub fn save(&self, _guard: &MutexGuard<'_, ()>, state: &PersistedState) -> Result<(), String> {
        let issues = state.consistency_issues();
        if !issues.is_empty() {
            return Err(format!(
                "refusing to save inconsistent state: {}",
                issues.join("; ")
            ));
        }
        save_persisted_state(&self.path, state).map_err(|e| e.to_string())?;
        let mut soon = lock(&self.soon);
        soon.pending = false;
        soon.deadline = None;
        Ok(())
    }

    pub fn save_soon(&self) {
        let mut soon = lock(&self.soon);
        soon.pending = true;
        if soon.deadline.is_none() {
            soon.deadline = Some(Instant::now() + SIZE_SAVE_DELAY);
            self.timer.notify_all();
        }
    }

    /// 遅らせている保存があるか（shutdown で今すぐ行うかの判断）。
    pub fn has_pending(&self) -> bool {
        lock(&self.soon).pending
    }

    pub fn log_failure(&self, message: &str) {
        self.log.error(&format!("failed to persist state: {message}"));
    }

    pub fn stop(&self) {
        lock(&self.soon).stopped = true;
        self.timer.notify_all();
    }

    /// 期限が来るまで眠り、来たら `flush` を呼ぶ。`stop` で戻る。専用のスレッドで呼ぶ。
    pub fn run_timer(&self, flush: impl Fn()) {
        loop {
            {
                let mut soon = lock(&self.soon);
                loop {
                    if soon.stopped {
                        return;
                    }
                    let Some(deadline) = soon.deadline else {
                        soon = self.timer.wait(soon).unwrap_or_else(PoisonError::into_inner);
                        continue;
                    };
                    let now = Instant::now();
                    if now >= deadline {
                        // 失敗しても次の save_soon で予約し直せるよう、保存の前に外す。
                        soon.deadline = None;
                        break;
                    }
                    soon = self
                        .timer
                        .wait_timeout(soon, deadline - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
            flush();
        }
    }
}
