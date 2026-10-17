//! ANSI 除去 + 行組み立て（Node 版の `AnsiLineAssembler`）。チャンク境界（UTF-8 の途中を含む）をまたぐ状態を保持する。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    EscInter,
    Csi,
    Osc,
    OscEsc,
    Str,
    StrEsc,
}

/// - CSI / OSC（BEL or ST 終端）/ DCS, SOS, PM, APC（ST 終端）/ 2 文字 ESC を除去
/// - `\n` `\r` `\t` 以外の C0 と DEL は捨てる
/// - `\r\n` は改行。単独 `\r` は「以降に書かれた文字で行を上書き」= 現在行バッファの破棄
/// - 行は `\n` で確定する。未確定の行は `pending` で覗ける。
#[derive(Debug)]
pub struct AnsiLineAssembler {
    /// 前のチャンクの末尾にあった、UTF-8 として未完成のバイト（最大 3 バイト）。
    carry: Vec<u8>,
    state: State,
    line: String,
    pending_cr: bool,
}

impl Default for AnsiLineAssembler {
    fn default() -> Self {
        Self {
            carry: Vec::new(),
            state: State::Ground,
            line: String::new(),
            pending_cr: false,
        }
    }
}

impl AnsiLineAssembler {
    /// 確定していない現在の行。
    pub fn pending(&self) -> &str {
        &self.line
    }

    /// バイト列を流し、確定した行（改行を除く）を返す。不正な UTF-8 は U+FFFD にする。
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut bytes = std::mem::take(&mut self.carry);
        bytes.extend_from_slice(chunk);
        let mut rest: &[u8] = &bytes;
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.run(text, &mut out);
                    break;
                }
                Err(e) => {
                    let (valid, after) = rest.split_at(e.valid_up_to());
                    self.run(&String::from_utf8_lossy(valid), &mut out);
                    match e.error_len() {
                        // 末尾で UTF-8 が途切れている。次のチャンクで続きが来る。
                        None => {
                            self.carry = after.to_vec();
                            break;
                        }
                        Some(len) => {
                            self.step('\u{FFFD}', &mut out);
                            rest = &after[len..];
                        }
                    }
                }
            }
        }
        out
    }

    fn run(&mut self, text: &str, out: &mut Vec<String>) {
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            if self.state == State::Ground {
                // 行に足すだけの文字（タブ、印字できる ASCII、ASCII 以外）の連続は、まとめて足す。
                let run = bytes[at..]
                    .iter()
                    .take_while(|&&b| b == 0x09 || (0x20..=0x7e).contains(&b) || b >= 0x80)
                    .count();
                if run > 0 {
                    if self.pending_cr {
                        self.line.clear();
                        self.pending_cr = false;
                    }
                    self.line.push_str(&text[at..at + run]);
                    at += run;
                    continue;
                }
            }
            let Some(ch) = text[at..].chars().next() else {
                break;
            };
            self.step(ch, out);
            at += ch.len_utf8();
        }
    }

    fn step(&mut self, ch: char, out: &mut Vec<String>) {
        let c = u32::from(ch);
        // 実端末は C0 を即時実行する。エスケープ列の途中でも改行 / CR は ground で処理する。
        if (c == 0x0a || c == 0x0d) && matches!(self.state, State::Esc | State::EscInter | State::Csi) {
            self.state = State::Ground;
        }
        match self.state {
            State::Ground => self.ground(ch, c, out),
            State::Esc => match ch {
                '[' => self.state = State::Csi,
                ']' => self.state = State::Osc,
                'P' | 'X' | '^' | '_' => self.state = State::Str,
                _ if (0x20..=0x2f).contains(&c) => self.state = State::EscInter,
                _ if c == 0x1b => self.state = State::Esc,
                _ if (0x30..=0x7e).contains(&c) => self.state = State::Ground,
                _ => {
                    // 範囲外（C0 / 非 ASCII など）: シーケンスを打ち切り、その文字を通常文字として再解釈
                    self.state = State::Ground;
                    self.step(ch, out);
                }
            },
            State::EscInter => {
                if (0x30..=0x7e).contains(&c) {
                    self.state = State::Ground;
                } else if c == 0x1b {
                    self.state = State::Esc;
                } else if c == 0x18 || c == 0x1a {
                    self.state = State::Ground;
                }
            }
            State::Csi => {
                if (0x40..=0x7e).contains(&c) {
                    self.state = State::Ground;
                } else if c == 0x1b {
                    self.state = State::Esc;
                } else if c == 0x18 || c == 0x1a {
                    self.state = State::Ground;
                }
            }
            State::Osc => match c {
                0x07 | 0x18 | 0x1a => self.state = State::Ground,
                0x1b => self.state = State::OscEsc,
                _ => {}
            },
            State::Str => match c {
                0x18 | 0x1a => self.state = State::Ground,
                0x1b => self.state = State::StrEsc,
                _ => {}
            },
            State::OscEsc | State::StrEsc => {
                // ESC \ (ST) で終端。それ以外は新しい ESC シーケンスの開始として再解釈する。
                if ch == '\\' {
                    self.state = State::Ground;
                } else {
                    self.state = State::Esc;
                    self.step(ch, out);
                }
            }
        }
    }

    fn ground(&mut self, ch: char, c: u32, out: &mut Vec<String>) {
        match c {
            0x1b => self.state = State::Esc,
            0x0a => {
                out.push(std::mem::take(&mut self.line));
                self.pending_cr = false;
            }
            0x0d => self.pending_cr = true,
            0x09 | 0x20..=0x7e | 0x80.. => {
                if self.pending_cr {
                    self.line.clear();
                    self.pending_cr = false;
                }
                self.line.push(ch);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(input: &str) -> Vec<String> {
        AnsiLineAssembler::default().push(input.as_bytes())
    }

    /// 1 バイトずつ流す。
    fn bytewise(input: &str) -> Vec<String> {
        let mut a = AnsiLineAssembler::default();
        input.as_bytes().iter().flat_map(|b| a.push(&[*b])).collect()
    }

    /// すべての 2 分割位置（UTF-8 の途中を含む）で流す。
    fn every_split(input: &str) -> Vec<Vec<String>> {
        let bytes = input.as_bytes();
        (0..=bytes.len())
            .map(|i| {
                let mut a = AnsiLineAssembler::default();
                let mut out = a.push(&bytes[..i]);
                out.extend(a.push(&bytes[i..]));
                out
            })
            .collect()
    }

    #[track_caller]
    fn check(input: &str, expected: &[&str]) {
        assert_eq!(whole(input), expected, "whole");
        assert_eq!(bytewise(input), expected, "bytewise");
        for split in every_split(input) {
            assert_eq!(split, expected, "split");
        }
    }

    #[test]
    fn plain_text_and_crlf_lf() {
        check("a\r\nb\nc\r\n", &["a", "b", "c"]);
    }

    #[test]
    fn csi_is_removed() {
        check("\x1b[1;31mred\x1b[0m \x1b[2A\x1b[?25lok\n", &["red ok"]);
    }

    #[test]
    fn osc_ends_with_bel_or_st() {
        check("\x1b]0;my title\x07visible\n", &["visible"]);
        check("\x1b]2;my title\x1b\\visible\n", &["visible"]);
    }

    #[test]
    fn string_sequences_end_with_st() {
        check(
            "\x1bPq#0;2;0;0;0\x1b\\A\x1b_Gdata\x1b\\B\x1b^pm\x1b\\C\x1bXsos\x1b\\D\n",
            &["ABCD"],
        );
    }

    #[test]
    fn two_character_escapes() {
        check("\x1b7a\x1b=b\x1b(Bc\x1b8\n", &["abc"]);
    }

    #[test]
    fn c0_controls_are_dropped_but_tab_stays() {
        check("a\x07b\x08c\x00\td\n", &["abc\td"]);
    }

    #[test]
    fn wide_characters_emoji_and_combining_pass_through() {
        check("日本語ＡＢＣ🎉é\n", &["日本語ＡＢＣ🎉é"]);
    }

    #[test]
    fn carriage_return_overwrites_the_line() {
        check("progress 10%\rprogress 50%\rdone\r\n", &["done"]);
        check("keep\r\nnext\r\n", &["keep", "next"]);
        check("abc\r\r\n", &["abc"]);
        check("old\r\x1b[Knew\n", &["new"]);
        check("0123456789abcdef\x1b[0mXYZ\rnew\x1b[0mline\n", &["newline"]);
    }

    #[test]
    fn an_esc_inside_osc_that_is_not_st_starts_a_new_sequence() {
        check("\x1b]0;t\x1b[31mX\x07Y\n", &["XY"]);
    }

    #[test]
    fn can_aborts_a_sequence() {
        check("\x1b[31\x18ok\n", &["ok"]);
    }

    #[test]
    fn an_unterminated_sequence_does_not_leak_into_the_next_line() {
        let mut a = AnsiLineAssembler::default();
        assert!(a.push(b"text\x1b]0;unterminated").is_empty());
        assert_eq!(a.pending(), "text");
    }

    #[test]
    fn markers_survive_sgr_and_split_writes() {
        check(
            "\x1b[1;32mAZITO_DO\x1b[0mNE_x_ab\x1b[0mcd\x1b[0m\r\n",
            &["AZITO_DONE_x_abcd"],
        );
    }

    #[test]
    fn newline_ends_a_stray_escape_so_the_next_marker_is_not_lost() {
        check("junk\x1b\nAZITO_DONE_x_1\r\n", &["junk", "AZITO_DONE_x_1"]);
        check("a\x1b[31\r\nAZITO_DONE_x_2\r\n", &["a", "AZITO_DONE_x_2"]);
        check("b\x1b(\nAZITO_DONE_x_3\n", &["b", "AZITO_DONE_x_3"]);
    }

    #[test]
    fn a_non_ascii_character_after_esc_is_reinterpreted_not_consumed() {
        check("\x1bあいう\n", &["あいう"]);
    }

    #[test]
    fn lines_split_by_sgr_and_surrogate_pairs_join_into_one() {
        check(
            "\x1b[31mred\x1b[0m 😀 \x1b[1mbold\x1b[0m tail\n",
            &["red 😀 bold tail"],
        );
    }

    #[test]
    fn pending_joins_the_unfinished_line_and_grows_with_later_pushes() {
        let mut a = AnsiLineAssembler::default();
        a.push(b"abc\x1b[0mde");
        assert_eq!(a.pending(), "abcde");
        assert_eq!(a.push(b"f\ng"), ["abcdef"]);
        assert_eq!(a.pending(), "g");
    }

    #[test]
    fn invalid_utf8_becomes_the_replacement_character() {
        let mut a = AnsiLineAssembler::default();
        assert_eq!(a.push(b"a\xffb\n"), ["a\u{FFFD}b"]);
    }
}
