//! ペインの行ストリーム: `pane.subscribe_lines` の購読元と、遅れた購読者を待つ pty の読み取り制御。
//! `pane` の子モジュールなので、`Pane` の内部（行リングと購読者の一覧）を直接扱う。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;

use super::{Pane, lock};
use crate::stream::{Next, Progress, StreamSource};

/// 行ストリームの購読者の遅れがリング容量のこの割合を超えたら、追いつくまで pty を読まない（子の書き込みを止める）。
/// 超えるまでは読み続け、リングが遅い購読者を吸収する。
const LAG_LIMIT_DIVISOR: u64 = 2;
/// 購読者を待っている間、通知が無くても遅れ（と、止まっているかどうか）を見直す間隔。
const THROTTLE_RECHECK: Duration = Duration::from_millis(100);

impl Pane {
    /// 行ストリームの購読者が、リングに追い越されそうなほど遅れている間は pty を読まない。
    /// 読まなければ子の書き込みが詰まるので、動いている（カーソルが進んでいる）購読者は取りこぼさない。
    /// 止まっている購読者（読んでいない）は待たず、リングに追い越されたら切断する。
    /// 閉じている途中・子の終了後は待たない（出力を読み切って終わらせる）。
    pub(super) fn wait_for_subscribers(&self) {
        loop {
            let seen = self.wakeup.begin();
            if !self.is_lagging() {
                self.wakeup.end();
                return;
            }
            self.wakeup.wait(seen, THROTTLE_RECHECK);
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
        progress.set_waker(Arc::clone(&self.0.wakeup));
        let mut subscribers = lock(&self.0.line_subscribers);
        // 終わった購読（失効した Weak）を残さない。
        subscribers.retain(|p| p.strong_count() > 0);
        subscribers.push(Arc::downgrade(progress));
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::super::{Pane, PaneOpenOptions};
    use super::*;

    fn pane() -> Arc<Pane> {
        Pane::spawn(PaneOpenOptions {
            cmd: vec!["sleep".into(), "30".into()],
            cwd: None,
            env: BTreeMap::new(),
            ephemeral_env: BTreeMap::new(),
            cols: None,
            rows: None,
            socket_path: Path::new("/tmp/unused.sock"),
            raw_ring_bytes: 1024,
            lines_ring_bytes: 1024,
        })
        .unwrap()
    }

    #[test]
    fn tracking_a_new_subscriber_drops_the_finished_ones() {
        let pane = pane();
        let source = LinesSource(Arc::clone(&pane));
        for _ in 0..100 {
            let progress = Arc::new(Progress::new(0));
            source.track(&progress);
            // 購読のタスクが終わった（progress が落ちた）。
        }
        let live = Arc::new(Progress::new(0));
        source.track(&live);
        assert_eq!(lock(&pane.line_subscribers).len(), 1);
        crate::pty::kill_tree(pane.pid);
    }
}
