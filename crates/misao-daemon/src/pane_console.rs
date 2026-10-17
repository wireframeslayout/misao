//! `Pane` の、端末の役としての振る舞い: 子の終了後の出力の読み切りと、端末への問い合わせ（DSR）への返事。
//! どちらも ConPTY（Windows）の事情による部分を含む。

use super::*;

impl Pane {
    /// 子の終了後、残りの出力を読み切るのを待つ。
    /// Unix は pty の EOF まで（孫が pty を握って EOF が来なければ `READER_DRAIN_TIMEOUT` で打ち切る）。
    /// Windows は EOF が来ないので、出力が `OUTPUT_QUIET` 途切れるまで（最長 `READER_DRAIN_TIMEOUT`）。
    pub(super) fn wait_for_output_drain(&self, drained: &mpsc::Receiver<()>) {
        if !cfg!(windows) {
            let _ = drained.recv_timeout(READER_DRAIN_TIMEOUT);
            return;
        }
        let started = Instant::now();
        loop {
            match drained.recv_timeout(Duration::from_millis(20)) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
            let quiet =
                started.elapsed() >= OUTPUT_QUIET && lock(&self.last_output).elapsed() >= OUTPUT_QUIET;
            if quiet || started.elapsed() >= READER_DRAIN_TIMEOUT {
                return;
            }
        }
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
}
