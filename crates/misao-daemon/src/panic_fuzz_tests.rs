//! Rust 版だけで確かめる「何を食わせても panic しない」ファズ。Node 版と比べない（比べるのは `node_diff_tests`）。
//! 乱数の種は固定で、回数は数秒で終わる程度。resize（極端な大きさを含む）、代替画面、カーソルの保存と復元、
//! 極端な引数、壊れた UTF-8、途中で切れた列を混ぜて、画面の不変条件（行数、カーソルの範囲）も確かめる。

use crate::pane_view::PaneView;
use crate::profile::builtin_profiles;
use crate::screen::Screen;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap_or(1)).unwrap_or(0)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NUMBERS: [&str; 10] = ["", "0", "1", "2", "3", "7", "80", "9999", "65535", "99999999999"];
const FINALS: [&str; 30] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "P", "S", "T", "X", "Z", "`", "a", "b",
    "d", "e", "f", "g", "r", "s", "u", "@", "m",
];
const MODES: [&str; 12] = [
    "1049", "47", "1047", "1048", "6", "7", "45", "4", "20", "2", "25", "2004",
];
const TEXT: [&str; 12] = [
    "abc",
    "日本語",
    "e\u{301}\u{301}",
    "✅😀",
    " ",
    "ｱｲｳ",
    "\u{200b}",
    "--",
    "\u{0}",
    "x\u{7f}y",
    "\u{301}",
    "│─┌",
];
const SIZES: [u64; 12] = [0, 1, 2, 3, 5, 12, 80, 200, 1024, 5000, u64::MAX, 40];

fn chunk(rng: &mut Rng) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for _ in 0..1 + rng.below(8) {
        let piece: String = match rng.below(14) {
            0..=2 => (*rng.pick(&TEXT)).to_string(),
            3 => "\r\n".to_string(),
            4..=6 => format!(
                "\x1b[{};{}{}",
                rng.pick(&NUMBERS),
                rng.pick(&NUMBERS),
                rng.pick(&FINALS)
            ),
            7 => format!("\x1b[{}{}", rng.pick(&NUMBERS), rng.pick(&FINALS)),
            8 => format!("\x1b[?{}{}", rng.pick(&MODES), rng.pick(&["h", "l"])),
            9 => (*rng.pick(&[
                "\x1b7", "\x1b8", "\x1bM", "\x1bD", "\x1bE", "\x1bH", "\x1bc", "\x1b[!p", "\x0e", "\x0f",
                "\t", "\x08",
            ]))
            .to_string(),
            10 => format!(
                "\x1b]{};{}\x07",
                rng.pick(&["0", "2", "1", "8", "52"]),
                rng.pick(&TEXT)
            ),
            11 => (*rng.pick(&[
                "\x1b(0",
                "\x1b)B",
                "\x1b#8",
                "\x1bP$q\x1b\\",
                "\x1b[>c",
                "\x1b[6n",
                "\x1b[?u",
                "\x1b_x\x1b\\",
            ]))
            .to_string(),
            12 => format!("\x1b[{}{}", rng.pick(&MODES), rng.pick(&["h", "l"])),
            _ => String::new(),
        };
        out.extend_from_slice(piece.as_bytes());
        // たまに壊れた UTF-8 や途中で切れた列を混ぜる
        match rng.below(16) {
            0 => out.push(0xff),
            1 => out.extend_from_slice(&[0xe6, 0x97]),
            2 => out.extend_from_slice(b"\x1b["),
            3 => out.push(0x1b),
            _ => {}
        }
    }
    out
}

fn clamped(size: u64, min: usize) -> usize {
    usize::try_from(size.min(1024)).unwrap_or(1024).max(min)
}

#[test]
fn the_screen_never_panics_and_keeps_its_invariants() {
    for seed in 1..=48_u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (mut cols, mut rows) = (*rng.pick(&SIZES), *rng.pick(&SIZES));
        let mut screen = Screen::new(cols, rows, rng.below(40));
        for step in 0..120 {
            if rng.below(12) == 0 {
                // 極端に大きい大きさは、たまにだけ
                let pick = |rng: &mut Rng| {
                    if rng.below(6) == 0 {
                        *rng.pick(&SIZES)
                    } else {
                        1 + rng.next() % 40
                    }
                };
                (cols, rows) = (pick(&mut rng), pick(&mut rng));
                screen.resize(cols, rows);
            } else {
                screen.feed(&chunk(&mut rng));
            }
            let lines = screen.rows();
            assert_eq!(
                lines.len(),
                clamped(rows, 1),
                "seed {seed} step {step}: row count"
            );
            let cursor = screen.cursor();
            assert!(
                cursor.y < lines.len(),
                "seed {seed} step {step}: cursor row {cursor:?}"
            );
            // 桁（x）は検査しない: xterm.js と同じく、幅 0 の文字を行末で受け続けると桁数を超えて進む。
        }
    }
}

#[test]
fn the_pane_view_never_panics_with_profiles_and_time_moving_on() {
    let profiles = builtin_profiles().unwrap_or_default();
    for seed in 1..=16_u64 {
        let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
        let mut view = PaneView::new(60, 12, rng.below(30), profiles.first().cloned());
        let mut now = 0_u64;
        for _ in 0..150 {
            now += 1 + rng.next() % 700;
            match rng.below(8) {
                0 => view.resize(*rng.pick(&SIZES), *rng.pick(&SIZES), now),
                1 => view.input(now),
                2 => {
                    view.set_profile(profiles.get(rng.below(3)).cloned(), now);
                }
                _ => {
                    view.output(&chunk(&mut rng), now);
                }
            }
            while let Some(deadline) = view.next_deadline().filter(|&d| d <= now) {
                view.advance(deadline);
            }
            let snapshot = view.snapshot();
            assert!(snapshot.cursor.y < snapshot.text.split('\n').count());
        }
        view.exited();
        assert_eq!(view.agent_state().0.as_str(), "exited");
    }
}
