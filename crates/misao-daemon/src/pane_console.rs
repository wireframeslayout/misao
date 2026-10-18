//! `Pane` の、端末の役としての振る舞い: 子の終了後の出力の読み切りと、端末への問い合わせ（DSR）への返事。
//! どちらも ConPTY（Windows）の事情による部分を含む。

use super::*;

impl Pane {
    /// リーダーの終了後、残りの出力を読み切るのを待つ。`pane.exited` と最終行の flush は、この後に 1 回だけ行う。
    ///
    /// Unix は pty の EOF まで（孫が pty を握って EOF が来なければ `READER_DRAIN_TIMEOUT` で打ち切る）。
    ///
    /// Windows の出力のパイプは ConPTY を閉じるまで EOF にならないので、次の順で止める:
    /// 1. Job の動作中プロセスが 0 になるのを待つ（孫の出力も読む）。`DESCENDANTS_WAIT` で打ち切る。
    /// 2. ConPTY を閉じる（ClosePseudoConsole が残りの出力を流し切ってから、出力のパイプを閉じる）。
    /// 3. reader が EOF を見て止まるのを待つ（`CONSOLE_CLOSE_WAIT`）。
    pub(super) fn finish_console(&self, drained: &mpsc::Receiver<()>) {
        if !cfg!(windows) {
            let _ = drained.recv_timeout(READER_DRAIN_TIMEOUT);
            return;
        }
        let started = Instant::now();
        while self.tree.group_state() == GroupState::Alive && started.elapsed() < DESCENDANTS_WAIT {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(lock(&self.master).take());
        let _ = drained.recv_timeout(CONSOLE_CLOSE_WAIT);
    }

    /// 端末への問い合わせ（子や ConPTY の出力に含まれる）への返事を、子の入力へ送る。アプリの入力ではないので、
    /// `pane.write` と違って入力のイベントも稼働判定の「入力あり」も起こさない。溜まりすぎていれば捨てる。
    pub(super) fn reply_to_terminal_query(&self, reply: Vec<u8>) {
        let input = lock(&self.input);
        let Some(tx) = input.as_ref() else { return };
        if self.check_input_room(reply.len()).is_err() {
            return;
        }
        self.input_queued.fetch_add(reply.len(), Ordering::AcqRel);
        let _ = tx.send(reply);
    }

    /// 再描画の候補（`repaint_filter`）の期限を見張るスレッドを始める。期限が来たら、次の出力を待たずに候補を行ストリームへ流す。
    /// ロックの順序は order -> inner。
    pub(super) fn watch_repaint_candidate(&self) {
        let me = self.me.clone();
        std::thread::spawn(move || {
            loop {
                let Some(pane) = me.upgrade() else { return };
                let wait = {
                    let _order = lock(&pane.order);
                    let mut inner = lock(&pane.inner);
                    let now = Instant::now();
                    let Some(deadline) = inner.repaint.deadline() else {
                        inner.repaint_timer = false;
                        return;
                    };
                    if now >= deadline {
                        let released = inner.repaint.expire(now);
                        let ts = now_iso();
                        let lines = inner.assembler.push(&released);
                        let any = !lines.is_empty();
                        for text in lines {
                            inner.push_line(text, ts.clone());
                        }
                        let head = any.then(|| inner.lines.head());
                        drop(inner);
                        if let Some(head) = head {
                            pane.lines_tx.send_replace(head);
                        }
                        continue;
                    }
                    deadline - now
                };
                drop(pane);
                std::thread::sleep(wait);
            }
        });
    }
}
