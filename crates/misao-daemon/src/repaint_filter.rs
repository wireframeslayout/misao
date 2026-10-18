//! ConPTY の resize 後の全画面の再描画を、行ストリームから除く。
//!
//! ConPTY は resize のたびに、画面全体を `ESC[H`（ホーム）から、各行を `ESC[K` + 改行で書き直す。ANSI を除いて行にすると、
//! すでに流した行が重複し、約 100 行の空行が入る（実機の記録: `docs/*/windows.md` §10.1）。
//! resize を適用した直後に 1 回だけ構えて（`arm`）、`ESC[H` で始まる候補のフレームを**確定するまでバッファする**。
//! 記録済みの再描画の形を満たしたときだけ捨てる:
//!
//! - 改行（LF）がすべて `ESC[K` `CR` の直後にあり、
//! - 改行が `rows-1` 個以下で、
//! - 最後の行の後ろのカーソル位置の復元（`rows-1` 個の改行の後の `ESC[<r>;<c>H`）か、カーソルの表示（`ESC[?25h`）で終わる。
//!
//! 形に合わない（アプリ自身の出力だった）、`FRAME_MAX` を過ぎた、行数を超えた、のどれかなら、バッファしたバイトを
//! すべて行の組み立てに渡し直して、構えを解く。生の出力のリング・画面モデル・稼働判定には元のバイト列がそのまま渡る。
//! 期限（`FRAME_MAX`）は pane が `deadline` / `expire` で見張る（次の出力を待たない）。終了時は `take_held` で取り出す。

use std::borrow::Cow;
use std::time::{Duration, Instant};

/// resize の後、再描画を待つ長さ。実機では resize から 800 ms 以内に収まる。
const ARM_WINDOW: Duration = Duration::from_secs(2);
/// フレームの開始から、確定しなければ捨てるのをやめて流すまでの長さ。
const FRAME_MAX: Duration = Duration::from_millis(500);
/// 候補のフレームをバッファする上限。超えたら再描画ではないとして流す。
const FRAME_MAX_BYTES: usize = 1024 * 1024;
/// CSI の途中でチャンクが切れたときに持ち越す上限。これを超える CSI はエスケープシーケンスではないとして流す。
const HELD_MAX: usize = 32;
/// 再描画の各行の終わり。
const LINE_END: &[u8] = b"\x1b[K\r\n";

/// バッファ中の候補のフレーム。
#[derive(Debug)]
struct Frame {
    started: Instant,
    line_feeds: u64,
    /// `ESC[H` から後ろの、受け取ったままのバイト列。
    bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct RepaintFilter {
    armed: Option<(Instant, u64)>,
    frame: Option<Frame>,
    /// 前のチャンクの末尾にあった、終わっていない CSI（`ESC [ ...`）。
    held: Vec<u8>,
}

fn is_final(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

/// ホームへの移動（`ESC[H` / `ESC[1;1H` / `ESC[;H` ...）か。params は `[` の後ろ、最終バイトの前。
fn is_home(params: &[u8]) -> bool {
    matches!(params, b"" | b"1" | b";" | b"1;" | b";1" | b"1;1")
}

impl RepaintFilter {
    /// resize を適用した。次の再描画のフレームを 1 つ除く。`rows` は新しい行数。
    pub fn arm(&mut self, rows: u64, now: Instant) {
        self.armed = Some((now + ARM_WINDOW, rows));
    }

    /// 確定しないまま保持している候補のバイト列を取り出し、構えを解く（pane の終了時に、最後の行まで流すために使う）。
    pub fn take_held(&mut self) -> Vec<u8> {
        self.armed = None;
        let mut out = self.frame.take().map(|f| f.bytes).unwrap_or_default();
        out.append(&mut self.held);
        out
    }

    /// 保持中の候補が期限を迎える時刻。候補が無ければ None（pane はこの時刻に `expire` を呼ぶ）。
    pub fn deadline(&self) -> Option<Instant> {
        self.frame.as_ref().map(|f| f.started + FRAME_MAX)
    }

    /// 期限を過ぎた候補があれば、流すべきバイト列を取り出して構えを解く（次の出力を待たずに行にするため）。
    pub fn expire(&mut self, now: Instant) -> Vec<u8> {
        match self.deadline() {
            Some(deadline) if now >= deadline => {
                let mut out = Vec::new();
                self.release(&mut out);
                out
            }
            _ => Vec::new(),
        }
    }

    /// 候補を流すことにして、構えを解く。
    fn release(&mut self, out: &mut Vec<u8>) {
        if let Some(frame) = self.frame.take() {
            out.extend_from_slice(&frame.bytes);
        }
        self.armed = None;
    }

    /// 行の組み立てに渡すバイト列を返す。構えていない間は何もしない（そのまま返す）。
    pub fn filter<'a>(&mut self, data: &'a [u8], now: Instant) -> Cow<'a, [u8]> {
        if let Some((until, _)) = self.armed
            && now > until
            && self.frame.is_none()
        {
            self.armed = None;
        }
        if self.armed.is_none() && self.held.is_empty() && self.frame.is_none() {
            return Cow::Borrowed(data);
        }
        let mut input = std::mem::take(&mut self.held);
        input.extend_from_slice(data);
        let mut out = Vec::with_capacity(input.len());
        let mut index = 0;
        while index < input.len() {
            if self
                .frame
                .as_ref()
                .is_some_and(|f| now.saturating_duration_since(f.started) > FRAME_MAX)
            {
                self.release(&mut out);
            }
            let byte = input[index];
            if byte != 0x1b {
                self.on_byte(byte, &mut out);
                index += 1;
                continue;
            }
            // ESC: CSI（`ESC [ params final`）だけを見る。
            let Some(&second) = input.get(index + 1) else {
                self.held = input[index..].to_vec();
                break;
            };
            if second != b'[' {
                self.on_bytes(&input[index..index + 2], &mut out);
                index += 2;
                continue;
            }
            let Some(length) = input[index + 2..].iter().position(|&b| is_final(b)) else {
                if input.len() - index <= HELD_MAX {
                    self.held = input[index..].to_vec();
                } else {
                    self.on_bytes(&input[index..], &mut out);
                }
                break;
            };
            let end = index + 2 + length;
            self.on_csi(&input[index + 2..end], input[end], now, &mut out);
            index = end + 1;
        }
        Cow::Owned(out)
    }

    /// フレームの中ならバッファへ、外なら行の組み立てへ。
    fn on_bytes(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            self.on_byte(b, out);
        }
    }

    fn on_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        let Some(frame) = &mut self.frame else {
            out.push(byte);
            return;
        };
        frame.bytes.push(byte);
        if byte == b'\n' {
            frame.line_feeds += 1;
            let rows = self.armed.map_or(0, |(_, rows)| rows);
            if !frame.bytes.ends_with(LINE_END) || frame.line_feeds >= rows {
                self.release(out);
            }
        } else if frame.bytes.len() > FRAME_MAX_BYTES {
            self.release(out);
        }
    }

    /// CSI を 1 つ見た。
    fn on_csi(&mut self, params: &[u8], final_byte: u8, now: Instant, out: &mut Vec<u8>) {
        let mut sequence = vec![0x1b, b'['];
        sequence.extend_from_slice(params);
        sequence.push(final_byte);
        let Some(frame) = &mut self.frame else {
            if final_byte == b'H' && is_home(params) && self.armed.is_some() {
                self.frame = Some(Frame {
                    started: now,
                    line_feeds: 0,
                    bytes: sequence,
                });
            } else {
                out.extend_from_slice(&sequence);
            }
            return;
        };
        frame.bytes.extend_from_slice(&sequence);
        let rows = self.armed.map_or(0, |(_, rows)| rows);
        let cursor_shown = final_byte == b'h' && params == b"?25";
        let cursor_restored = final_byte == b'H' && frame.line_feeds + 1 == rows;
        if cursor_shown || cursor_restored {
            // 形を満たした再描画: 捨てて、構えを解く。
            self.frame = None;
            self.armed = None;
        } else if frame.bytes.len() > FRAME_MAX_BYTES {
            self.release(out);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn run(filter: &mut RepaintFilter, data: &[u8], now: Instant) -> Vec<u8> {
        filter.filter(data, now).into_owned()
    }

    fn frame(rows: usize, cup: &str) -> String {
        format!(
            "\x1b[?25l\x1b[H{}\x1b[K{cup}\x1b[?25h",
            "text\x1b[K\r\n".repeat(rows - 1)
        )
    }

    #[test]
    fn nothing_changes_while_not_armed() {
        let mut filter = RepaintFilter::default();
        let data = frame(5, "\x1b[2;3H");
        assert_eq!(run(&mut filter, data.as_bytes(), Instant::now()), data.as_bytes());
    }

    #[test]
    fn the_frame_after_a_resize_is_dropped_but_the_surroundings_are_kept() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        let input = format!("before\r\n{}after\r\n", frame(5, "\x1b[2;3H"));
        let out = String::from_utf8(run(&mut filter, input.as_bytes(), now)).unwrap();
        assert_eq!(out, "before\r\n\x1b[?25l\x1b[?25hafter\r\n");
        // 1 回きり: 次の ESC[H はそのまま通る
        let again = frame(5, "\x1b[2;3H");
        assert_eq!(run(&mut filter, again.as_bytes(), now), again.as_bytes());
    }

    #[test]
    fn a_frame_split_at_every_byte_is_still_dropped() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(4, now);
        let input = format!("a\r\n{}b\r\n", frame(4, "\x1b[1;46H"));
        let mut out = Vec::new();
        for byte in input.as_bytes() {
            out.extend(run(&mut filter, std::slice::from_ref(byte), now));
        }
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("a\r\n") && text.ends_with("b\r\n"), "{text:?}");
        assert!(!text.contains("text"), "{text:?}");
    }

    #[test]
    fn the_frame_ends_at_the_cursor_show_when_the_row_count_differs() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(10, now);
        let input = format!("{}tail\r\n", frame(3, ""));
        let out = String::from_utf8(run(&mut filter, input.as_bytes(), now)).unwrap();
        assert_eq!(out, "\x1b[?25ltail\r\n");
    }

    #[test]
    fn app_output_that_starts_with_home_right_after_a_resize_is_not_dropped() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        // 再描画の形 (各行が ESC[K CR LF) ではないので、行の組み立てへ全部渡る
        let out = run(&mut filter, b"\x1b[Himportant\r\nnext\r\n", now);
        assert_eq!(out, b"\x1b[Himportant\r\nnext\r\n");
        // 構えは解けている: 次のホームも通る
        assert_eq!(run(&mut filter, b"\x1b[Hx\r\n", now), b"\x1b[Hx\r\n");
    }

    #[test]
    fn an_app_line_split_across_chunks_is_not_dropped_either() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        let mut out = run(&mut filter, b"\x1b[Hbig", now);
        assert_eq!(out, b"", "a candidate is held until it is decided");
        out.extend(run(&mut filter, b"news\r\n", now));
        assert_eq!(out, b"\x1b[Hbignews\r\n");
    }

    #[test]
    fn a_frame_with_too_many_lines_is_not_a_repaint() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(3, now);
        let input = frame(6, "\x1b[1;1H");
        assert_eq!(run(&mut filter, input.as_bytes(), now), input.as_bytes());
    }

    #[test]
    fn an_unfinished_candidate_is_released_after_the_time_limit_or_at_exit() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        // 形は合っているが終わりの印が来ない (カーソルを隠したまま): 500 ms 後の出力で、候補ごと流れる
        assert_eq!(run(&mut filter, b"\x1b[Hxx\x1b[K\r\n", now), b"");
        let later = now + FRAME_MAX + Duration::from_millis(1);
        assert_eq!(
            run(&mut filter, b"real\r\n", later),
            b"\x1b[Hxx\x1b[K\r\nreal\r\n"
        );
        // 終了時は take_held で全部取り出せる
        filter.arm(5, later);
        assert_eq!(run(&mut filter, b"\x1b[Hlast", later), b"");
        assert_eq!(filter.take_held(), b"\x1b[Hlast");
        assert_eq!(run(&mut filter, b"\x1b[Hok", later), b"\x1b[Hok", "disarmed");
    }

    #[test]
    fn an_expired_arm_is_forgotten() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        let expired = now + ARM_WINDOW + Duration::from_millis(1);
        assert_eq!(run(&mut filter, b"\x1b[Hok\r\n", expired), b"\x1b[Hok\r\n");
    }

    #[test]
    fn other_sequences_and_a_long_unfinished_csi_pass_through() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        assert_eq!(
            run(&mut filter, b"\x1b[31mred\x1b[0m\x1b]0;t\x07", now),
            b"\x1b[31mred\x1b[0m\x1b]0;t\x07"
        );
        let long = format!("\x1b[{}", "9".repeat(64));
        assert_eq!(run(&mut filter, long.as_bytes(), now), long.as_bytes());
    }

    #[test]
    fn the_recorded_windows_repaints_leave_no_duplicate_lines() {
        use crate::ansi::AnsiLineAssembler;

        for (name, text) in [
            (
                "cmd",
                include_str!("../tests/fixtures/windows/conpty-cmd-idle-resize.jsonl"),
            ),
            (
                "powershell",
                include_str!("../tests/fixtures/windows/conpty-ps-idle-resize.jsonl"),
            ),
        ] {
            let mut filter = RepaintFilter::default();
            let mut assembler = AnsiLineAssembler::default();
            let (mut lines, mut unfiltered) = (Vec::new(), Vec::new());
            let mut plain = AnsiLineAssembler::default();
            let start = Instant::now();
            for record in text.lines().skip(1) {
                let value: serde_json::Value = serde_json::from_str(record).unwrap();
                let at = start + Duration::from_millis(value["t"].as_u64().unwrap());
                if let Some(size) = value["resize"].as_array() {
                    filter.arm(size[1].as_u64().unwrap(), at);
                } else if let Some(data) = value["d"].as_str() {
                    lines.extend(assembler.push(&filter.filter(data.as_bytes(), at)));
                    unfiltered.extend(plain.push(data.as_bytes()));
                }
            }
            // 記録の最後の `exit` を除いて、banner と prompt が 1 回ずつ。空行は prompt の前の 1 行だけ。
            assert!(
                unfiltered.len() > lines.len() + 50,
                "{name}: the repaints did produce noise"
            );
            let count = |needle: &str| lines.iter().filter(|l| l.contains(needle)).count();
            assert!(count("misao-win-test") <= 1, "{name}: {lines:?}");
            assert!(
                lines.iter().filter(|l| l.is_empty()).count() <= 1,
                "{name}: {lines:?}"
            );
            assert!(lines.len() <= 6, "{name}: {lines:?}");
        }
    }

    #[test]
    fn expire_releases_a_candidate_at_its_deadline_without_more_output() {
        let mut filter = RepaintFilter::default();
        let now = Instant::now();
        filter.arm(5, now);
        assert_eq!(filter.deadline(), None);
        assert_eq!(run(&mut filter, b"\x1b[Hpartial\x1b[K\r\n", now), b"");
        let deadline = filter.deadline().unwrap();
        assert_eq!(filter.expire(deadline - Duration::from_millis(1)), b"");
        assert_eq!(filter.expire(deadline), b"\x1b[Hpartial\x1b[K\r\n");
        assert_eq!(filter.deadline(), None);
        assert_eq!(run(&mut filter, b"\x1b[Hx", deadline), b"\x1b[Hx", "disarmed");
    }
}
