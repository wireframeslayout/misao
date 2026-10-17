//! セルの属性（文字飾りと色）。画面の文字には効かないが、attach の snapshot が見た目を再現するために持つ。
//! 1 つの `u64` に詰める: 下位 8 bit が飾り、その上に前景色と背景色（それぞれ 2 bit の種類 + 24 bit の値）。

use vte::Params;

const BOLD: u64 = 1;
const DIM: u64 = 1 << 1;
const ITALIC: u64 = 1 << 2;
const UNDERLINE: u64 = 1 << 3;
const BLINK: u64 = 1 << 4;
const INVERSE: u64 = 1 << 5;
const INVISIBLE: u64 = 1 << 6;
const STRIKETHROUGH: u64 = 1 << 7;

const COLOR_BITS: u32 = 26;
const COLOR_MASK: u64 = (1 << COLOR_BITS) - 1;
const FG_SHIFT: u32 = 8;
const BG_SHIFT: u32 = FG_SHIFT + COLOR_BITS;
const MODE_SHIFT: u32 = 24;

/// 色の種類。値は 24 bit の下に置く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    /// 0-255 のパレット（0-15 は標準色）。
    Palette(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn pack(self) -> u64 {
        match self {
            Self::Default => 0,
            Self::Palette(n) => (1 << MODE_SHIFT) | u64::from(n),
            Self::Rgb(r, g, b) => {
                (2 << MODE_SHIFT) | (u64::from(r) << 16) | (u64::from(g) << 8) | u64::from(b)
            }
        }
    }

    fn unpack(bits: u64) -> Self {
        let value = bits & 0xFF_FFFF;
        match bits >> MODE_SHIFT {
            1 => Self::Palette(u8::try_from(value & 0xFF).unwrap_or(0)),
            2 => Self::Rgb(
                u8::try_from((value >> 16) & 0xFF).unwrap_or(0),
                u8::try_from((value >> 8) & 0xFF).unwrap_or(0),
                u8::try_from(value & 0xFF).unwrap_or(0),
            ),
            _ => Self::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attr(u64);

impl Attr {
    pub const DEFAULT: Self = Self(0);

    pub fn is_default(self) -> bool {
        self.0 == 0
    }

    fn has(self, flag: u64) -> bool {
        self.0 & flag != 0
    }

    pub fn fg(self) -> Color {
        Color::unpack((self.0 >> FG_SHIFT) & COLOR_MASK)
    }

    pub fn bg(self) -> Color {
        Color::unpack((self.0 >> BG_SHIFT) & COLOR_MASK)
    }

    fn with_flag(self, flag: u64, on: bool) -> Self {
        Self(if on { self.0 | flag } else { self.0 & !flag })
    }

    fn with_fg(self, color: Color) -> Self {
        Self((self.0 & !(COLOR_MASK << FG_SHIFT)) | (color.pack() << FG_SHIFT))
    }

    fn with_bg(self, color: Color) -> Self {
        Self((self.0 & !(COLOR_MASK << BG_SHIFT)) | (color.pack() << BG_SHIFT))
    }

    /// 消去したセルが持つ属性: 背景色だけを引き継ぐ（xterm.js の `eraseAttrData`）。
    pub fn erase(self) -> Self {
        Self::DEFAULT.with_bg(self.bg())
    }

    /// 属性を既定に戻したうえで再設定する SGR（`\x1b[0;1;38;2;r;g;bm` の形）。Node 版の `sgrFor` と同じ。
    pub fn sgr_sequence(self) -> String {
        let mut parts: Vec<String> = vec!["0".to_string()];
        for (flag, code) in [
            (BOLD, "1"),
            (DIM, "2"),
            (ITALIC, "3"),
            (UNDERLINE, "4"),
            (BLINK, "5"),
            (INVERSE, "7"),
            (INVISIBLE, "8"),
            (STRIKETHROUGH, "9"),
        ] {
            if self.has(flag) {
                parts.push(code.to_string());
            }
        }
        color_params(&mut parts, self.fg(), 30);
        color_params(&mut parts, self.bg(), 40);
        format!("\x1b[{}m", parts.join(";"))
    }

    /// `CSI ... m`。xterm.js の `charAttributes` のうち、snapshot が再現する範囲（飾りと色）。
    pub fn apply_sgr(&mut self, params: &Params) {
        let groups: Vec<Vec<u16>> = params.iter().map(<[u16]>::to_vec).collect();
        if groups.is_empty() {
            *self = Self::DEFAULT;
            return;
        }
        let mut i = 0;
        while i < groups.len() {
            let group = &groups[i];
            let code = group.first().copied().unwrap_or(0);
            i += 1;
            match code {
                0 => *self = Self::DEFAULT,
                1 => *self = self.with_flag(BOLD, true),
                2 => *self = self.with_flag(DIM, true),
                3 => *self = self.with_flag(ITALIC, true),
                4 => *self = self.with_flag(UNDERLINE, group.get(1) != Some(&0)),
                5 => *self = self.with_flag(BLINK, true),
                7 => *self = self.with_flag(INVERSE, true),
                8 => *self = self.with_flag(INVISIBLE, true),
                9 => *self = self.with_flag(STRIKETHROUGH, true),
                21 => *self = self.with_flag(UNDERLINE, true),
                22 => *self = self.with_flag(BOLD | DIM, false),
                23 => *self = self.with_flag(ITALIC, false),
                24 => *self = self.with_flag(UNDERLINE, false),
                25 => *self = self.with_flag(BLINK, false),
                27 => *self = self.with_flag(INVERSE, false),
                28 => *self = self.with_flag(INVISIBLE, false),
                29 => *self = self.with_flag(STRIKETHROUGH, false),
                30..=37 => *self = self.with_fg(palette(code - 30)),
                39 => *self = self.with_fg(Color::Default),
                40..=47 => *self = self.with_bg(palette(code - 40)),
                49 => *self = self.with_bg(Color::Default),
                90..=97 => *self = self.with_fg(palette(code - 90 + 8)),
                100..=107 => *self = self.with_bg(palette(code - 100 + 8)),
                38 | 48 | 58 => {
                    let (color, used) = extended_color(&groups, i - 1);
                    i = i - 1 + used;
                    match (code, color) {
                        (38, Some(color)) => *self = self.with_fg(color),
                        (48, Some(color)) => *self = self.with_bg(color),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn palette(n: u16) -> Color {
    Color::Palette(u8::try_from(n).unwrap_or(0))
}

fn color_params(parts: &mut Vec<String>, color: Color, base: u16) {
    match color {
        Color::Default => {}
        Color::Rgb(r, g, b) => parts.extend([
            (base + 8).to_string(),
            "2".into(),
            r.to_string(),
            g.to_string(),
            b.to_string(),
        ]),
        Color::Palette(n) if n < 8 => parts.push((base + u16::from(n)).to_string()),
        Color::Palette(n) if n < 16 => parts.push((base + 60 + u16::from(n) - 8).to_string()),
        Color::Palette(n) => parts.extend([(base + 8).to_string(), "5".into(), n.to_string()]),
    }
}

/// `38` / `48` / `58` の引数を読む。`groups[at]` がその先頭。`(色, 使ったグループ数)` を返す。
/// `38;5;n` / `38;2;r;g;b`（`;` 区切り）と `38:5:n` / `38:2:r:g:b` / `38:2::r:g:b`（`:` 区切り）の両方を受ける。
fn extended_color(groups: &[Vec<u16>], at: usize) -> (Option<Color>, usize) {
    let head = &groups[at];
    if head.len() > 1 {
        // `:` 区切り。1 つのグループに全部入っている。
        let color = match head[1] {
            5 => head.get(2).map(|&n| palette(n)),
            // 色空間の欄を必ず 1 つ飛ばす（xterm.js と同じ。`38:2:r:g:b` は r を色空間として読み、足りない値は 0）。
            2 => {
                let component = |i: usize| head.get(i).copied().unwrap_or(0);
                rgb_of(&[component(3), component(4), component(5)])
            }
            _ => None,
        };
        return (color, 1);
    }
    let value = |offset: usize| groups.get(at + offset).and_then(|g| g.first().copied());
    match value(1) {
        Some(5) => (value(2).map(palette), 3),
        Some(2) => {
            let rgb: Vec<u16> = (2..5).filter_map(value).collect();
            (rgb_of(&rgb), 5)
        }
        _ => (None, 1),
    }
}

fn rgb_of(rgb: &[u16]) -> Option<Color> {
    let [r, g, b] = rgb else {
        return None;
    };
    let byte = |n: u16| u8::try_from(n.min(255)).unwrap_or(255);
    Some(Color::Rgb(byte(*r), byte(*g), byte(*b)))
}

#[cfg(test)]
mod tests {
    use vte::{Params, Parser, Perform};

    use super::*;

    #[derive(Default)]
    struct Collect(Attr);

    impl Perform for Collect {
        fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, action: char) {
            if action == 'm' {
                self.0.apply_sgr(params);
            }
        }
    }

    fn sgr(input: &str) -> Attr {
        let mut parser = Parser::new();
        let mut collect = Collect::default();
        parser.advance(&mut collect, input.as_bytes());
        collect.0
    }

    #[test]
    fn decorations_and_colors_round_trip_through_the_replay_sequence() {
        let attr = sgr("\x1b[1;3;4;7;31;104m");
        assert_eq!(attr.sgr_sequence(), "\x1b[0;1;3;4;7;31;104m");
        assert_eq!(
            sgr("\x1b[38;5;200;48;2;1;2;3m").sgr_sequence(),
            "\x1b[0;38;5;200;48;2;1;2;3m"
        );
        assert_eq!(
            sgr("\x1b[38:2::10:20:30m").sgr_sequence(),
            "\x1b[0;38;2;10;20;30m"
        );
        assert_eq!(sgr("\x1b[38:5:9m").sgr_sequence(), "\x1b[0;91m");
        assert_eq!(
            sgr("\x1b[48:2:9:8:7m").sgr_sequence(),
            "\x1b[0;48;2;8;7;0m",
            "the colour space slot is always skipped"
        );
    }

    #[test]
    fn reset_and_selective_off_codes() {
        assert!(sgr("\x1b[1;31m\x1b[m").is_default());
        assert_eq!(sgr("\x1b[1;2;3m\x1b[22;23m").sgr_sequence(), "\x1b[0m");
        assert_eq!(sgr("\x1b[31;41m\x1b[39m").sgr_sequence(), "\x1b[0;41m");
    }

    #[test]
    fn erase_keeps_only_the_background() {
        let erased = sgr("\x1b[1;31;44m").erase();
        assert_eq!(erased.sgr_sequence(), "\x1b[0;44m");
        assert!(sgr("\x1b[1;31m").erase().is_default());
    }

    #[test]
    fn unknown_or_truncated_extended_colors_are_ignored() {
        assert!(sgr("\x1b[38;5m").is_default());
        assert!(sgr("\x1b[38;2;1;2m").is_default());
        assert_eq!(sgr("\x1b[58;5;1;1m").sgr_sequence(), "\x1b[0;1m");
    }
}
