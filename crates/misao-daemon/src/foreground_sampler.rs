//! 前面のプログラムの定期調査（デーモンに 1 つ）。前面のプログラムが変わったら profile を選び直すために、
//! 起動コマンドだけでは profile が決まらない pane（シェルなど）を一定間隔でまとめて調べ、各 pane に配る。
//! macOS では全 pane を 1 回の `ps` で問い合わせる（pane ごとに起動しない）。

use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::Duration;

use crate::foreground::read_foreground_commands;
use crate::pane::Pane;

/// 調べる間隔。Linux は `/proc` を読むだけなので短く、他の Unix は `ps` を起動するので長くする。
#[cfg(target_os = "linux")]
const INTERVAL: Duration = Duration::from_secs(1);
#[cfg(not(target_os = "linux"))]
const INTERVAL: Duration = Duration::from_secs(5);

struct Shared {
    panes: Mutex<Vec<Weak<Pane>>>,
    stopped: Mutex<bool>,
    wake: Condvar,
}

pub struct ForegroundSampler {
    shared: Arc<Shared>,
}

impl ForegroundSampler {
    pub fn start() -> Self {
        let shared = Arc::new(Shared {
            panes: Mutex::new(Vec::new()),
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        std::thread::spawn(move || run(&thread_shared));
        Self { shared }
    }

    /// 調査の対象に加える。pane が捨てられたら自然に外れる。
    pub fn register(&self, pane: &Arc<Pane>) {
        if pane.needs_foreground_probe() {
            self.shared
                .panes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Arc::downgrade(pane));
        }
    }
}

impl Drop for ForegroundSampler {
    fn drop(&mut self) {
        *self.shared.stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.shared.wake.notify_all();
    }
}

fn run(shared: &Shared) {
    loop {
        let stopped = shared.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (stopped, _) = shared
            .wake
            .wait_timeout_while(stopped, INTERVAL, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        if *stopped {
            return;
        }
        drop(stopped);
        sample(shared);
    }
}

/// 対象の pane を 1 回でまとめて調べ、結果を配る。
fn sample(shared: &Shared) {
    let panes: Vec<Arc<Pane>> = {
        let mut list = shared.panes.lock().unwrap_or_else(PoisonError::into_inner);
        list.retain(|weak| weak.upgrade().is_some_and(|pane| pane.needs_foreground_probe()));
        list.iter().filter_map(Weak::upgrade).collect()
    };
    let queries: Vec<_> = panes.iter().filter_map(|pane| pane.foreground_query()).collect();
    if queries.len() != panes.len() {
        // 調べる間に終了した pane がある。次の回に任せる。
        return;
    }
    for (pane, name) in panes.iter().zip(read_foreground_commands(&queries)) {
        pane.apply_foreground(name.as_deref());
    }
}
