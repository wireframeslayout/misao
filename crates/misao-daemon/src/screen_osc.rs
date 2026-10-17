//! OSC（`ESC ] ... BEL` / `ESC ] ... ESC \`）の長さの上限。
//!
//! `vte` は `std` 機能のとき OSC の中身を上限なしに貯める。子が終端のない OSC を送り続けても、メモリを使い切らないように、
//! パーサーに渡す前に OSC の長さを数える。`MAX_OSC_BYTES` に達したら CAN（0x18）を差し込んで OSC を中止させ、終端までの残りを捨てる。
//! 中止された OSC は `vte` から（上限の長さの中身で）届くので、画面側（`screen_perform`）が `MAX_OSC_BYTES` 以上の OSC を捨てる。
//! 結果は xterm.js が上限を超えた OSC を捨てるのと同じ: タイトルは変わらない（切り詰めたタイトルにはならない）。

/// OSC の中身の上限（バイト）。xterm.js は 10MB。タイトルなど実際に使う OSC は数 KB に収まる。
pub const MAX_OSC_BYTES: usize = 1024 * 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
/// 8 ビットの ST（U+009C）の UTF-8 は `C2 9C`。
const C2: u8 = 0xc2;
const ST_TAIL: u8 = 0x9c;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// OSC の外で ESC を受けた直後。
    GroundEsc,
    /// OSC の中身を数えている（これまでのバイト数）。
    Body(usize),
    /// OSC の中身で `C2` を受けた直後（次が `9C` なら ST。そうでなければ `C2` は中身の一部）。
    BodyC2(usize),
    /// OSC の中で ESC を受けた直後（`\` なら終端、`]` なら次の OSC）。
    BodyEsc,
    /// 上限に達したので、終端まで捨てている。
    Skip,
    SkipC2,
    SkipEsc,
}

/// パーサーへ渡すもの。
#[derive(Debug, PartialEq, Eq)]
pub enum Segment {
    /// そのまま渡す。
    Bytes(Vec<u8>),
    /// OSC を中止させる 1 バイト（CAN / SUB）。`vte` は中止の前に OSC を dispatch するので、画面側はこの 1 バイトを渡す間、
    /// OSC の dispatch を捨てる（キャンセルされた OSC は何も起こさない。xterm.js と同じ）。
    Cancel(u8),
}

#[derive(Debug)]
pub struct OscGuard {
    state: State,
    limit: usize,
}

impl Default for OscGuard {
    fn default() -> Self {
        Self::new(MAX_OSC_BYTES)
    }
}

/// 出力の組み立て。
struct Out(Vec<Segment>);

impl Out {
    fn bytes(&mut self, bytes: &[u8]) {
        if let Some(Segment::Bytes(last)) = self.0.last_mut() {
            last.extend_from_slice(bytes);
        } else {
            self.0.push(Segment::Bytes(bytes.to_vec()));
        }
    }

    fn cancel(&mut self, byte: u8) {
        self.0.push(Segment::Cancel(byte));
    }
}

impl OscGuard {
    pub fn new(limit: usize) -> Self {
        Self {
            state: State::Ground,
            limit,
        }
    }

    /// `data` のうち、パーサーへ渡すもの。状態は呼び出しをまたいで持つ。
    pub fn filter(&mut self, data: &[u8]) -> Vec<Segment> {
        let mut out = Out(Vec::new());
        for &byte in data {
            self.step(byte, &mut out);
        }
        out.0
    }

    fn after_esc(byte: u8, esc: State) -> State {
        match byte {
            b']' => State::Body(0),
            ESC => esc,
            _ => State::Ground,
        }
    }

    fn step(&mut self, byte: u8, out: &mut Out) {
        match self.state {
            State::Ground => {
                self.state = if byte == ESC {
                    State::GroundEsc
                } else {
                    State::Ground
                };
                out.bytes(&[byte]);
            }
            State::GroundEsc => {
                self.state = Self::after_esc(byte, State::GroundEsc);
                out.bytes(&[byte]);
            }
            State::Body(n) | State::BodyC2(n) => {
                self.step_body(byte, n, matches!(self.state, State::BodyC2(_)), out)
            }
            State::BodyEsc => {
                self.state = Self::after_esc(byte, State::BodyEsc);
                out.bytes(&[byte]);
            }
            State::Skip => self.step_skip(byte, out),
            State::SkipC2 => {
                if byte == ST_TAIL {
                    self.state = State::Ground;
                } else {
                    self.state = State::Skip;
                    self.step_skip(byte, out);
                }
            }
            State::SkipEsc => {
                self.state = Self::after_esc(byte, State::SkipEsc);
                out.bytes(&[byte]);
            }
        }
    }

    /// OSC の中身の 1 バイト。`pending_c2` なら、直前の `C2` はまだ渡していない。
    fn step_body(&mut self, byte: u8, n: usize, pending_c2: bool, out: &mut Out) {
        if pending_c2 && byte == ST_TAIL {
            // 8 ビットの ST: `ESC \` として渡して OSC を終える。
            self.state = State::Ground;
            out.bytes(&[ESC, b'\\']);
            return;
        }
        let mut n = n;
        if pending_c2 {
            out.bytes(&[C2]);
            n += 1;
        }
        match byte {
            BEL => {
                self.state = State::Ground;
                out.bytes(&[byte]);
            }
            CAN | SUB => {
                self.state = State::Ground;
                out.cancel(byte);
            }
            ESC => {
                self.state = State::BodyEsc;
                out.bytes(&[byte]);
            }
            _ if n >= self.limit => {
                self.state = State::Skip;
                out.cancel(CAN);
            }
            C2 => self.state = State::BodyC2(n),
            _ => {
                self.state = State::Body(n + 1);
                out.bytes(&[byte]);
            }
        }
    }

    fn step_skip(&mut self, byte: u8, out: &mut Out) {
        match byte {
            BEL | CAN | SUB => {
                self.state = State::Ground;
                out.bytes(&[byte]);
            }
            ESC => {
                self.state = State::SkipEsc;
                out.bytes(&[byte]);
            }
            C2 => self.state = State::SkipC2,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(segments: &[Segment]) -> Vec<u8> {
        segments
            .iter()
            .flat_map(|s| match s {
                Segment::Bytes(b) => b.clone(),
                Segment::Cancel(c) => vec![*c],
            })
            .collect()
    }

    #[test]
    fn short_osc_sequences_and_text_pass_through_unchanged() {
        let mut guard = OscGuard::new(8);
        let input = "a\x1b]0;hi\x07b\x1b]2;x\x1b\\c\x1b[1mz日".as_bytes();
        let out = guard.filter(input);
        assert_eq!(out, vec![Segment::Bytes(input.to_vec())]);
    }

    #[test]
    fn an_over_long_osc_is_aborted_and_its_rest_is_dropped_up_to_the_terminator() {
        let mut guard = OscGuard::new(4);
        let out = guard.filter(b"x\x1b]0;abcdefghij\x07y");
        assert_eq!(
            out,
            vec![
                Segment::Bytes(b"x\x1b]0;ab".to_vec()),
                Segment::Cancel(CAN),
                Segment::Bytes(b"\x07y".to_vec())
            ]
        );
    }

    #[test]
    fn the_state_carries_over_between_chunks_and_a_new_osc_can_follow_the_terminator() {
        let mut guard = OscGuard::new(3);
        let mut out = bytes(&guard.filter(b"\x1b]0;aaaaaaaa"));
        out.extend(bytes(&guard.filter(b"aaaa\x1b\\\x1b]2;o\x07")));
        assert_eq!(out, b"\x1b]0;a\x18\x1b\\\x1b]2;o\x07".to_vec());
    }

    #[test]
    fn can_and_sub_inside_an_osc_are_passed_as_cancel_segments() {
        let mut guard = OscGuard::new(100);
        let out = guard.filter(b"\x1b]0;abc\x18x\x1b]2;d\x1ay");
        assert_eq!(
            out,
            vec![
                Segment::Bytes(b"\x1b]0;abc".to_vec()),
                Segment::Cancel(CAN),
                Segment::Bytes(b"x\x1b]2;d".to_vec()),
                Segment::Cancel(SUB),
                Segment::Bytes(b"y".to_vec()),
            ]
        );
    }

    #[test]
    fn the_8_bit_st_ends_an_osc_but_other_c2_sequences_stay_in_the_title() {
        let mut guard = OscGuard::new(100);
        assert_eq!(
            bytes(&guard.filter(b"\x1b]0;ab\xc2\x9cz")),
            b"\x1b]0;ab\x1b\\z".to_vec()
        );
        let mut guard = OscGuard::new(100);
        // C2 A0（U+00A0）は中身。チャンクの切れ目をまたいでも同じ。
        let mut out = bytes(&guard.filter(b"\x1b]0;a\xc2"));
        out.extend(bytes(&guard.filter(b"\xa0b\x07")));
        assert_eq!(out, b"\x1b]0;a\xc2\xa0b\x07".to_vec());
        // 上限を超えて捨てている間も、8 ビットの ST で終わる
        let mut guard = OscGuard::new(1);
        assert_eq!(
            bytes(&guard.filter(b"\x1b]0;abcdef\xc2\x9cz")),
            b"\x1b]0\x18z".to_vec()
        );
    }
}
