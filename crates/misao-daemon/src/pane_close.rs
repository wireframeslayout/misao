//! `Pane` の close（SIGHUP → 猶予 → SIGKILL、グループに残ったプロセスの掃除、リーダーの回収）。
//! 片付けは最初の呼び出しが起動する独立したタスクが 1 回だけ行う。

use super::*;

impl Pane {
    /// SIGHUP → 3 秒 → SIGKILL（プロセスグループ全体）。リーダーの終了と、グループに残ったプロセスの
    /// 掃除、リーダーの回収まで済んだら戻る。リーダーが既に（自然に）終了していても、グループに残ったプロセスがいれば
    /// 同じ手順で止める。片付けは 1 回だけ行い、並行して呼ばれた分と後から呼ばれた分は、その完了を待つだけ。
    /// 片付けは最初の呼び出しが起動する独立したタスクで行うので、呼び出し側がキャンセルされても続く
    /// （デーモンの終了で接続のタスクが abort された後の `shutdown` が、SIGHUP と猶予を数え直さない）。
    pub async fn close(self: &Arc<Self>) {
        if !self.close_started.swap(true, Ordering::AcqRel) {
            self.refuse_attach();
            let pane = Arc::clone(self);
            tokio::spawn(async move {
                pane.run_close().await;
                pane.closed_tx.send_replace(true);
            });
        }
        let mut done = self.closed_tx.subscribe();
        let _ = done.wait_for(|finished| *finished).await;
    }

    async fn run_close(&self) {
        let pid = self.pid;
        let started = Instant::now();
        *lock(&self.closing) = Some(started);
        self.wakeup.notify();
        let deadline = started + KILL_GRACE;
        let was_running = self.state() == RunState::Running;
        // リーダーを回収していない（まだ誰も回収していない最初の片付け）なら pid は自分のもの。
        // 回収する環境でなければ、終了時に残りがいたときだけ触れる。
        let owns_group = was_running || pty::LEADER_HELD || self.survivors_at_exit.load(Ordering::Acquire);
        if owns_group && (was_running || pty::group_state(pid) != GroupState::Empty) {
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
        if owns_group {
            self.sweep_group(deadline).await;
        }
        self.reap_leader();
    }

    /// 回収を保留していたリーダーを回収する（グループの片付けの後）。成功（または回収済み）を確かめてから回収済みにする。
    pub(super) fn reap_leader(&self) {
        if self.state() == RunState::Exited
            && !self.reaped.load(Ordering::Acquire)
            && pty::reap(self.pid).is_ok()
        {
            self.reaped.store(true, Ordering::Release);
        }
    }

    /// グループに残ったプロセスが消えるのを待ち、猶予が過ぎても残っていれば SIGKILL する。
    /// 調べられない（`Unknown`）ときは消えたとは言えないので、期限まで待って SIGKILL してから先へ進む
    /// （リーダーを回収するまで pgid は自分のものなので、無関係なグループには当たらない）。
    async fn sweep_group(&self, deadline: Instant) {
        let pid = self.pid;
        while pty::group_state(pid) != GroupState::Empty {
            if Instant::now() >= deadline {
                blocking(move || pty::kill_tree(pid)).await;
                let give_up = Instant::now() + GROUP_KILL_WAIT;
                while pty::group_state(pid) != GroupState::Empty && Instant::now() < give_up {
                    tokio::time::sleep(GROUP_POLL_INTERVAL).await;
                }
                return;
            }
            tokio::time::sleep(GROUP_POLL_INTERVAL).await;
        }
    }
}
