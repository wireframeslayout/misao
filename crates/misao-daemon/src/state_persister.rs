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
    /// `save_soon` のたびに進む世代。スナップショットを取る前の世代と比べて、保存中に予約が入ったかを知る。
    generation: u64,
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

    /// 今の世代。スナップショットを取る前（書き込みの順序づけの lock を取った後）に読んで `save` に渡す。
    pub fn generation(&self) -> u64 {
        lock(&self.soon).generation
    }

    /// 壊れた状態（存在しない window を指す pane など）は書かずに Err にする。成功したら遅らせていた保存を取り消す
    /// （失敗したとき、またはスナップショットの後に `save_soon` が呼ばれていたときは予約を残し、次の `flush` で保存する）。
    /// `generation` はスナップショットを取る前の `generation()`。
    pub fn save(
        &self,
        _guard: &MutexGuard<'_, ()>,
        generation: u64,
        state: &PersistedState,
    ) -> Result<(), String> {
        let issues = state.consistency_issues();
        if !issues.is_empty() {
            return Err(format!(
                "refusing to save inconsistent state: {}",
                issues.join("; ")
            ));
        }
        save_persisted_state(&self.path, state).map_err(|e| e.to_string())?;
        let mut soon = lock(&self.soon);
        if soon.generation == generation {
            soon.pending = false;
            soon.deadline = None;
        } else if soon.deadline.is_none() {
            // 保存中に予約が入った（その変更はスナップショットに入っていない）。期限を作り直して必ず保存する。
            soon.deadline = Some(Instant::now() + SIZE_SAVE_DELAY);
            self.timer.notify_all();
        }
        Ok(())
    }

    pub fn save_soon(&self) {
        let mut soon = lock(&self.soon);
        soon.pending = true;
        soon.generation += 1;
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::log::{LogLevel, Logger};

    #[test]
    fn a_save_soon_during_a_save_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let persister = Arc::new(StatePersister::new(
            &dir.path().join("persistence.json"),
            Logger::new(LogLevel::Error, |_| {}),
        ));
        let snapshot_taken = Arc::new(Barrier::new(2));
        let resized = Arc::new(Barrier::new(2));
        let saver = {
            let (persister, snapshot_taken, resized) = (
                Arc::clone(&persister),
                Arc::clone(&snapshot_taken),
                Arc::clone(&resized),
            );
            std::thread::spawn(move || {
                let guard = persister.write_guard();
                let generation = persister.generation();
                let state = PersistedState::empty(); // スナップショット
                snapshot_taken.wait(); // ここで resize の save_soon が入る
                resized.wait();
                persister.save(&guard, generation, &state).unwrap();
            })
        };
        snapshot_taken.wait();
        persister.save_soon();
        resized.wait();
        saver.join().unwrap();
        assert!(
            persister.has_pending(),
            "the resize after the snapshot must still be saved later"
        );
        // 新しいスナップショットで保存すれば予約は消える
        let guard = persister.write_guard();
        let generation = persister.generation();
        persister
            .save(&guard, generation, &PersistedState::empty())
            .unwrap();
        assert!(!persister.has_pending());
    }
}
