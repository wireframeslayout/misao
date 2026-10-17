//! 画面モデル（Node 版の `@xterm/headless` + `screen.ts` の `viewportText`）。
//! 端末の解釈は `vte`（パーサー）と `screen_emulator`（xterm.js と同じ意味の画面）で行う。
//! ここでは「出力を食わせる」「大きさを変える」「見えている行・カーソル・代替画面を読む」だけを提供する。

use vte::Parser;

use crate::screen_emulator::{Emulator, MIN_COLS, MIN_ROWS};
use crate::screen_osc::{OscGuard, Segment};

/// 画面の大きさの上限。プロトコルは安全な整数まで受けるが、格子はセルごとにメモリを使うので、
/// 画面モデルだけをここで丸める（`pane.info` とイベントには元の値を返す）。
const MAX_DIMENSION: u64 = 1024;

fn clamp(value: u64, min: usize) -> usize {
    usize::try_from(value.min(MAX_DIMENSION))
        .unwrap_or(usize::MAX)
        .max(min)
}

/// カーソルの位置（0 始まり）。行末に書いた直後は、xterm.js と同じく桁数（画面の右端の外）になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
}

pub struct Screen {
    parser: Parser,
    emulator: Emulator,
    osc: OscGuard,
}

impl Screen {
    pub fn new(cols: u64, rows: u64, scrollback: usize) -> Self {
        Self {
            parser: Parser::new(),
            osc: OscGuard::default(),
            emulator: Emulator::new(clamp(cols, MIN_COLS), clamp(rows, MIN_ROWS), scrollback),
        }
    }

    /// 出力を端末に食わせる。この出力で設定されたタイトルを順に返す（同じ値の再設定も 1 件ずつ）。
    pub fn feed(&mut self, data: &[u8]) -> Vec<String> {
        for segment in self.osc.filter(data) {
            match segment {
                Segment::Bytes(bytes) => self.parser.advance(&mut self.emulator, &bytes),
                Segment::Cancel(byte) => {
                    self.emulator.discard_osc = true;
                    self.parser.advance(&mut self.emulator, &[byte]);
                    self.emulator.discard_osc = false;
                }
            }
        }
        self.emulator.flush_run();
        std::mem::take(&mut self.emulator.titles)
    }

    pub fn resize(&mut self, cols: u64, rows: u64) {
        // xterm.js は、今と同じ大きさ（丸める前の値で比べる）なら何もしない。スクロール領域も戻さない。
        if u64::try_from(self.emulator.cols) == Ok(cols) && u64::try_from(self.emulator.rows) == Ok(rows) {
            return;
        }
        self.emulator.resize(clamp(cols, MIN_COLS), clamp(rows, MIN_ROWS));
    }

    pub fn is_alt_screen(&self) -> bool {
        self.emulator.is_alt()
    }

    pub fn cursor(&self) -> Cursor {
        let buffer = self.emulator.buf_ref();
        Cursor {
            x: buffer.x,
            y: buffer.y,
        }
    }

    /// 再生用のエスケープ列（attach の snapshot）。
    pub fn serialize_snapshot(&self) -> String {
        self.emulator.serialize_snapshot()
    }

    /// 見えている各行の文字（行末の空のセルは除く）。
    pub fn rows(&self) -> Vec<String> {
        let buffer = self.emulator.buf_ref();
        (0..self.emulator.rows)
            .map(|y| buffer.row(y).translate(true))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(screen: &Screen) -> String {
        screen.rows().join("\n")
    }

    #[test]
    fn text_wraps_at_the_right_edge_and_the_cursor_waits_outside_the_last_column() {
        let mut screen = Screen::new(5, 3, 10);
        screen.feed(b"abcde");
        assert_eq!(screen.cursor(), Cursor { x: 5, y: 0 });
        screen.feed(b"f");
        assert_eq!(
            (text(&screen), screen.cursor()),
            ("abcde\nf\n".to_string(), Cursor { x: 1, y: 1 })
        );
    }

    #[test]
    fn titles_are_returned_per_chunk_including_repeats() {
        let mut screen = Screen::new(10, 2, 10);
        assert_eq!(
            screen.feed(b"\x1b]0;a\x07\x1b]2;a\x1b\\x"),
            vec!["a".to_string(), "a".to_string()]
        );
        assert_eq!(screen.feed(b"\x1b]0;\x07"), vec![String::new()]);
        assert_eq!(screen.feed(b"\x1b]1;icon\x07"), Vec::<String>::new());
    }

    #[test]
    fn the_alternate_screen_is_separate_and_restored() {
        let mut screen = Screen::new(10, 2, 10);
        screen.feed(b"main\x1b[?1049h\x1b[2J\x1b[Halt");
        assert!(screen.is_alt_screen());
        assert_eq!(text(&screen), "alt\n");
        screen.feed(b"\x1b[?1049l");
        assert!(!screen.is_alt_screen());
        assert_eq!(text(&screen), "main\n");
    }

    #[test]
    fn explicitly_written_spaces_are_kept_but_erased_cells_are_not() {
        let mut screen = Screen::new(10, 2, 10);
        screen.feed(b"ab  \r\ncd\x1b[Kx  ");
        assert_eq!(text(&screen), "ab  \ncdx  ");
        screen.feed(b"\x1b[2K");
        assert_eq!(text(&screen), "ab  \n");
    }

    #[test]
    fn emoji_are_one_column_wide_like_xterm_js_with_unicode_6_and_cjk_are_two() {
        let mut screen = Screen::new(10, 2, 10);
        screen.feed("✅日".as_bytes());
        assert_eq!(screen.cursor(), Cursor { x: 3, y: 0 });
    }

    #[test]
    fn repeating_with_the_largest_count_is_bounded_by_the_screen_size() {
        let mut screen = Screen::new(80, 24, 100);
        let started = std::time::Instant::now();
        for _ in 0..200 {
            screen.feed(b"x\x1b[65535b");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(screen.rows().len(), 24);
    }

    #[test]
    fn an_endless_osc_is_discarded_without_buffering_it_and_the_text_after_it_survives() {
        let mut screen = Screen::new(10, 2, 10);
        let mut titles = screen.feed(b"\x1b]0;");
        let chunk = vec![b'a'; 64 * 1024];
        for _ in 0..40 {
            titles.extend(screen.feed(&chunk));
        }
        titles.extend(screen.feed(b"\x07ok\x1b]0;fine\x07"));
        assert_eq!(titles, vec!["fine".to_string()]);
        assert_eq!(text(&screen), "ok\n");
    }

    #[test]
    fn a_cancelled_osc_sets_no_title_and_the_8_bit_st_ends_one() {
        let mut screen = Screen::new(10, 2, 10);
        // CAN / SUB で中止された OSC は何も起こさない（xterm.js と同じ）。中止の後の文字は画面に出る。
        assert_eq!(
            screen.feed(b"\x1b]0;lost\x18ab\x1b]2;lost\x1acd"),
            Vec::<String>::new()
        );
        assert_eq!(text(&screen), "abcd\n");
        // 8 ビットの ST（U+009C）で終わる
        assert_eq!(
            screen.feed("\x1b]0;kept\u{9c}ef".as_bytes()),
            vec!["kept".to_string()]
        );
        assert_eq!(text(&screen), "abcdef\n");
    }

    #[test]
    fn sizes_are_clamped_for_the_model_only() {
        let mut screen = Screen::new(1, 0, 10);
        assert_eq!(screen.rows().len(), 1);
        screen.resize(u64::MAX, u64::MAX);
        assert_eq!(screen.rows().len(), 1024);
    }
}
