//! 稼働判定の title 段。OSC タイトルの見た目だけで working / idle を決める（Node 版の `TitleActivity`）。
//! スピナーを一度でも見た pane は、タイトルを自分で更新するエージェントとみなし、
//! スピナーでない空でないタイトルを idle と判定する。

use crate::byte_activity::ActivityVerdict;

/// スピナーは約 1 秒で切り替わる。これだけ更新が無ければ止まったとみなして意見を持たない。
const SPINNER_STALE_MS: u64 = 3000;

/// 汎用のスピナー: 点字 / ◐◑◒◓ / ✻✶✽✢∗ の 1 文字 + 空白で始まるタイトル。
fn is_spinner_title(title: &str) -> bool {
    let mut chars = title.chars();
    let is_glyph = |c: char| matches!(c, '\u{2800}'..='\u{28FF}' | '\u{25D0}'..='\u{25D3}' | '✻' | '✶' | '✽' | '✢' | '∗');
    matches!((chars.next(), chars.next()), (Some(first), Some(' ')) if is_glyph(first))
}

#[derive(Debug, Default)]
pub struct TitleActivity {
    title: String,
    updated_at: u64,
    has_seen_spinner: bool,
}

impl TitleActivity {
    pub fn set_title(&mut self, title: &str, now: u64) {
        self.title = title.to_string();
        self.updated_at = now;
        if is_spinner_title(title) {
            self.has_seen_spinner = true;
        }
    }

    pub fn verdict(&self, now: u64) -> Option<ActivityVerdict> {
        if is_spinner_title(&self.title) {
            return (now.saturating_sub(self.updated_at) < SPINNER_STALE_MS)
                .then_some(ActivityVerdict::Working);
        }
        (self.has_seen_spinner && !self.title.is_empty()).then_some(ActivityVerdict::Idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_titles_need_a_glyph_and_a_space() {
        for title in ["⠋ build", "◐ x", "✻ Thinking", "∗ a"] {
            assert!(is_spinner_title(title), "{title}");
        }
        for title in ["⠋build", "build", "", "⠋", " ⠋ x", "✳ Claude"] {
            assert!(!is_spinner_title(title), "{title}");
        }
    }

    #[test]
    fn a_stale_spinner_has_no_opinion_and_a_plain_title_after_a_spinner_is_idle() {
        let mut title = TitleActivity::default();
        assert_eq!(title.verdict(0), None);
        title.set_title("⠋ build", 1000);
        assert_eq!(title.verdict(3999), Some(ActivityVerdict::Working));
        assert_eq!(title.verdict(4000), None);
        title.set_title("done", 5000);
        assert_eq!(title.verdict(5000), Some(ActivityVerdict::Idle));
        title.set_title("", 6000);
        assert_eq!(title.verdict(6000), None);
    }
}
