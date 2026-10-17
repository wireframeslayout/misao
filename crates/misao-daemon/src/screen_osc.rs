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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// OSC の外で ESC を受けた直後。
    GroundEsc,
    /// OSC の中身を数えている（これまでのバイト数）。
    Body(usize),
    /// OSC の中で ESC を受けた直後（`\` なら終端、`]` なら次の OSC）。
    BodyEsc,
    /// 上限に達したので、終端まで捨てている。
    Skip,
    SkipEsc,
}

enum Action {
    Pass,
    Drop,
    Abort,
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

impl OscGuard {
    pub fn new(limit: usize) -> Self {
        Self {
            state: State::Ground,
            limit,
        }
    }

    /// `data` のうち、パーサーへ渡すもの。状態は呼び出しをまたいで持つ。
    pub fn filter(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &byte in data {
            match self.step(byte) {
                Action::Pass => out.push(byte),
                Action::Drop => {}
                Action::Abort => out.push(CAN),
            }
        }
        out
    }

    fn step(&mut self, byte: u8) -> Action {
        // ESC の次の `]` は OSC の始まり（どの状態でも）。ESC の次のそれ以外は、OSC を終えて普通の列に戻る。
        let after_esc = |byte: u8, esc: State| match byte {
            b']' => State::Body(0),
            ESC => esc,
            _ => State::Ground,
        };
        match self.state {
            State::Ground | State::GroundEsc => {
                self.state = if byte == ESC {
                    State::GroundEsc
                } else if self.state == State::GroundEsc {
                    after_esc(byte, State::GroundEsc)
                } else {
                    State::Ground
                };
                Action::Pass
            }
            State::Body(n) => match byte {
                BEL | CAN | SUB => {
                    self.state = State::Ground;
                    Action::Pass
                }
                ESC => {
                    self.state = State::BodyEsc;
                    Action::Pass
                }
                _ if n >= self.limit => {
                    self.state = State::Skip;
                    Action::Abort
                }
                _ => {
                    self.state = State::Body(n + 1);
                    Action::Pass
                }
            },
            State::BodyEsc => {
                self.state = after_esc(byte, State::BodyEsc);
                Action::Pass
            }
            State::Skip => match byte {
                BEL | CAN | SUB => {
                    self.state = State::Ground;
                    Action::Pass
                }
                ESC => {
                    self.state = State::SkipEsc;
                    Action::Pass
                }
                _ => Action::Drop,
            },
            State::SkipEsc => {
                self.state = after_esc(byte, State::SkipEsc);
                Action::Pass
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_osc_sequences_and_text_pass_through_unchanged() {
        let mut guard = OscGuard::new(8);
        let input = b"a\x1b]0;hi\x07b\x1b]2;x\x1b\\c\x1b[1mz";
        assert_eq!(guard.filter(input), input);
    }

    #[test]
    fn an_over_long_osc_is_aborted_and_its_rest_is_dropped_up_to_the_terminator() {
        let mut guard = OscGuard::new(4);
        let out = guard.filter(b"x\x1b]0;abcdefghij\x07y");
        assert_eq!(out, b"x\x1b]0;ab\x18\x07y".to_vec());
    }

    #[test]
    fn the_state_carries_over_between_chunks_and_a_new_osc_can_follow_the_terminator() {
        let mut guard = OscGuard::new(3);
        let mut out = guard.filter(b"\x1b]0;aaaaaaaa");
        out.extend(guard.filter(b"aaaa\x1b\\\x1b]2;o\x07"));
        assert_eq!(out, b"\x1b]0;a\x18\x1b\\\x1b]2;o\x07".to_vec());
    }
}
