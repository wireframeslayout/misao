//! 稼働判定の最後の段（bytes）。出力量だけで working / idle を決める（Node 版の `ByteActivity`）。
//! 時刻（pane の起動からのミリ秒）は引数で受け取り、タイマーは持たない。tick は呼び出し側が 1 秒ごとに呼ぶ。

/// 出力量を数えるスライディングウィンドウの長さ。
const WINDOW_MS: u64 = 3000;
/// ウィンドウ内の合計がこの量以上のティックを「活動あり」とする。
const ACTIVE_THRESHOLD_BYTES: usize = 200;
/// idle → working に必要な、活動ありのティックの連続数（エコーの除外）。
const ACTIVE_CONSECUTIVE_TICKS: u32 = 2;
/// 最後の活動ありのティックからこの時間静かなら idle。
const IDLE_AFTER_MS: u64 = 5000;
/// 入力の後の出力はキー入力のエコー / 入力欄の再描画なので数えない。
const INPUT_GRACE_MS: u64 = 500;
/// resize の後の出力は全画面の再描画なので数えない。
const RESIZE_GRACE_MS: u64 = 800;

/// bytes / title 段の意見（`None` は意見なし）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityVerdict {
    Working,
    Idle,
}

#[derive(Debug)]
pub struct ByteActivity {
    samples: Vec<(u64, usize)>,
    state: Option<ActivityVerdict>,
    above_streak: u32,
    has_fresh_bytes: bool,
    last_above: u64,
    input_grace_until: u64,
    resize_grace_until: u64,
}

impl ByteActivity {
    pub fn new(started_at: u64) -> Self {
        Self {
            samples: Vec::new(),
            state: None,
            above_streak: 0,
            has_fresh_bytes: false,
            last_above: started_at,
            input_grace_until: 0,
            resize_grace_until: 0,
        }
    }

    pub fn notify_input(&mut self, now: u64) {
        self.input_grace_until = now + INPUT_GRACE_MS;
    }

    pub fn notify_resize(&mut self, now: u64) {
        self.resize_grace_until = now + RESIZE_GRACE_MS;
    }

    pub fn record(&mut self, bytes: usize, now: u64) {
        if now < self.resize_grace_until || now < self.input_grace_until {
            return;
        }
        self.samples.push((now, bytes));
        self.has_fresh_bytes = true;
    }

    /// 一定間隔（1 秒）で呼ぶ。現在の意見を返す。開いてから最初の活動が無いまま `IDLE_AFTER_MS` が過ぎるまでは意見なし。
    pub fn tick(&mut self, now: u64) -> Option<ActivityVerdict> {
        let cutoff = now.saturating_sub(WINDOW_MS);
        self.samples.retain(|&(ts, _)| ts >= cutoff);
        let sum: usize = self.samples.iter().map(|&(_, bytes)| bytes).sum();
        let is_above = sum >= ACTIVE_THRESHOLD_BYTES && self.has_fresh_bytes;
        self.has_fresh_bytes = false;
        self.above_streak = if is_above { self.above_streak + 1 } else { 0 };
        if is_above {
            self.last_above = now;
        }
        let quiet = now.saturating_sub(self.last_above) >= IDLE_AFTER_MS;
        if self.state == Some(ActivityVerdict::Working) {
            if !is_above && quiet {
                self.state = Some(ActivityVerdict::Idle);
            }
        } else if self.above_streak >= ACTIVE_CONSECUTIVE_TICKS {
            self.state = Some(ActivityVerdict::Working);
        } else if quiet {
            self.state = Some(ActivityVerdict::Idle);
        }
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_without_an_opinion_until_the_first_quiet_period() {
        let mut bytes = ByteActivity::new(0);
        assert_eq!(bytes.tick(1000), None);
        assert_eq!(bytes.tick(4000), None);
        assert_eq!(bytes.tick(5000), Some(ActivityVerdict::Idle));
    }

    #[test]
    fn needs_two_consecutive_active_ticks_to_work() {
        let mut bytes = ByteActivity::new(0);
        bytes.record(300, 500);
        assert_eq!(bytes.tick(1000), None);
        bytes.record(10, 1500);
        assert_eq!(bytes.tick(2000), Some(ActivityVerdict::Working));
    }

    #[test]
    fn input_and_resize_grace_hide_the_echo() {
        let mut bytes = ByteActivity::new(0);
        bytes.notify_input(1000);
        bytes.record(500, 1499);
        bytes.notify_resize(1000);
        bytes.record(500, 1799);
        assert!(!bytes.has_fresh_bytes);
        bytes.record(500, 1800);
        assert!(bytes.has_fresh_bytes);
    }

    #[test]
    fn goes_idle_after_five_quiet_seconds() {
        let mut bytes = ByteActivity::new(0);
        for (tick, at) in [(1000, 500), (2000, 1500)] {
            bytes.record(300, at);
            bytes.tick(tick);
        }
        assert_eq!(bytes.tick(2000), Some(ActivityVerdict::Working));
        assert_eq!(bytes.tick(6999), Some(ActivityVerdict::Working));
        assert_eq!(bytes.tick(7000), Some(ActivityVerdict::Idle));
    }
}
