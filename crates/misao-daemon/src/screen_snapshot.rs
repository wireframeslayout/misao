//! 画面の「現在の見た目」を、再生用のエスケープ列にする（Node 版の `serializeSnapshot`）。attach の `replay: snapshot` が使う。
//! 見えている行（viewport）だけで、スクロールバックは含めない。代替画面なら、代替画面に入ってから描く。

use crate::screen_emulator::{Emulator, MouseMode};

impl Emulator {
    pub fn serialize_snapshot(&self) -> String {
        let buffer = self.buf_ref();
        let mut out = String::new();
        if self.is_alt() {
            out.push_str("\x1b[?1049h");
        }
        out.push_str("\x1b[0m\x1b[2J\x1b[H");
        for y in 0..self.rows {
            let line = buffer.row(y);
            // 末尾の「既定属性の空白」は書かない（行末は EL で消す）。
            let end = (0..line.len())
                .rev()
                .find(|&x| {
                    let cell = line.cell(x);
                    cell.width() != 0
                        && (cell.has_content() && line.string_at(x) != " " || !cell.attr().is_default())
                })
                .map_or(0, |last| last + 1);
            out.push_str(&format!("\x1b[{};1H", y + 1));
            let mut current = String::new();
            for x in 0..end {
                let cell = line.cell(x);
                if cell.width() == 0 {
                    continue;
                }
                let sgr = cell.attr().sgr_sequence();
                if sgr != current {
                    out.push_str(&sgr);
                    current = sgr;
                }
                let text = line.string_at(x);
                out.push_str(if text.is_empty() { " " } else { &text });
            }
            out.push_str("\x1b[0m\x1b[K");
        }
        // 描画の後に設定する（挿入モードなどが描画自体に効かないように）。
        // 既知の制約: カーソルの表示 / 非表示（DECTCEM）とスクロール領域（DECSTBM）は復元しない（Node 版と同じ）。
        let input = self.input;
        if input.app_cursor_keys {
            out.push_str("\x1b[?1h");
        }
        if input.app_keypad {
            out.push_str("\x1b=");
        }
        if self.insert_mode {
            out.push_str("\x1b[4h");
        }
        if !self.wraparound {
            out.push_str("\x1b[?7l");
        }
        if input.send_focus {
            out.push_str("\x1b[?1004h");
        }
        if input.bracketed_paste {
            out.push_str("\x1b[?2004h");
        }
        out.push_str(match input.mouse {
            MouseMode::None => "",
            MouseMode::X10 => "\x1b[?9h",
            MouseMode::Vt200 => "\x1b[?1000h",
            MouseMode::Drag => "\x1b[?1002h",
            MouseMode::Any => "\x1b[?1003h",
        });
        // DECOM はカーソルをホームに戻すので、最後のカーソル位置指定より前に置く。
        if self.origin {
            out.push_str("\x1b[?6h");
        }
        out.push_str(&format!("\x1b[{};{}H", buffer.y + 1, buffer.x + 1));
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::screen::Screen;

    fn snapshot(input: &[u8]) -> String {
        let mut screen = Screen::new(10, 3, 10);
        screen.feed(input);
        screen.serialize_snapshot()
    }

    #[test]
    fn plain_text_is_drawn_row_by_row_with_the_cursor_last() {
        assert_eq!(
            snapshot(b"ab\r\ncd"),
            "\x1b[0m\x1b[2J\x1b[H\x1b[1;1H\x1b[0mab\x1b[0m\x1b[K\x1b[2;1H\x1b[0mcd\x1b[0m\x1b[K\x1b[3;1H\x1b[0m\x1b[K\x1b[2;3H"
        );
    }

    #[test]
    fn colors_are_replayed_and_a_colored_blank_tail_is_kept() {
        let text = snapshot(b"\x1b[31mred\x1b[44m  \x1b[0m");
        assert!(text.contains("\x1b[0;31mred\x1b[0;31;44m  "), "{text:?}");
        // 背景色つきの消去も再現する（BCE）。
        let erased = snapshot(b"\x1b[44m\x1b[K\x1b[0m");
        assert!(
            erased.contains("\x1b[1;1H\x1b[0;44m          \x1b[0m\x1b[K"),
            "{erased:?}"
        );
    }

    #[test]
    fn the_alternate_screen_and_modes_are_restored() {
        let text = snapshot(b"\x1b[?1049h\x1b[?2004h\x1b[?1000h\x1b[?1h\x1b=x");
        assert!(text.starts_with("\x1b[?1049h\x1b[0m\x1b[2J\x1b[H"));
        assert!(
            text.ends_with("\x1b[?1h\x1b=\x1b[?2004h\x1b[?1000h\x1b[1;2H"),
            "{text:?}"
        );
        let reset = snapshot(b"\x1b[?2004h\x1b[?2004l\x1b[?1000h\x1b[?1002h\x1b[?1002l");
        assert!(
            reset.ends_with("\x1b[1;1H") && !reset.contains("?2004h") && !reset.contains("?100"),
            "{reset:?}"
        );
    }

    #[test]
    fn wide_characters_and_combining_marks_are_written_once() {
        let text = snapshot("日e\u{301}".as_bytes());
        assert!(text.contains("\x1b[0m日e\u{301}\x1b[0m\x1b[K"), "{text:?}");
    }
}
