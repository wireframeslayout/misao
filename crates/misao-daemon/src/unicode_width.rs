//! 文字幅（xterm.js の Unicode 6 の規則）。表は `scripts/gen-unicode-width.mjs` が `@xterm/headless` から書き出す。
//! 絵文字などは Unicode 6 では幅 1 になる（xterm.js の既定と同じ。幅 2 にする拡張は入れていない）。

use crate::unicode_width_table::NON_SINGLE_WIDTH;

/// コードポイントの表示幅（0 = 結合文字・制御文字、1、2 = 全角）。
pub fn char_width(code: u32) -> u8 {
    NON_SINGLE_WIDTH
        .binary_search_by(|&(start, end, _)| {
            if code < start {
                std::cmp::Ordering::Greater
            } else if code > end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .map_or(1, |i| NON_SINGLE_WIDTH[i].2)
}

/// xterm.js の `charProperties` の値。幅（2 ビット）と「直前の文字に結合するか」（1 ビット）を持つ。
/// 0 は「直前に文字が無い」（結合の相手がいない）を表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharProps(u32);

impl CharProps {
    fn new(width: u8, should_join: bool) -> Self {
        Self((u32::from(width & 3) << 1) | u32::from(should_join))
    }

    pub fn width(self) -> u8 {
        u8::try_from((self.0 >> 1) & 3).unwrap_or(0)
    }

    pub fn should_join(self) -> bool {
        self.0 & 1 == 1
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// `preceding` は直前の文字の `CharProps`。幅 0 の文字は、直前に幅のある文字があれば結合する。
pub fn char_properties(code: u32, preceding: CharProps) -> CharProps {
    let mut width = char_width(code);
    let mut should_join = width == 0 && !preceding.is_empty();
    if should_join {
        let old_width = preceding.width();
        if old_width == 0 {
            should_join = false;
        } else if old_width > width {
            width = old_width;
        }
    }
    CharProps::new(width, should_join)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_follow_unicode_6() {
        assert_eq!(char_width('a' as u32), 1);
        assert_eq!(char_width('日' as u32), 2);
        assert_eq!(char_width(0x0301), 0);
        assert_eq!(char_width(0x200B), 0);
        assert_eq!(char_width('✅' as u32), 1);
        assert_eq!(char_width(0x1F600), 1);
        assert_eq!(char_width(0x20000), 2);
    }

    #[test]
    fn a_combining_mark_joins_only_after_a_character_with_width() {
        let base = char_properties('e' as u32, CharProps::default());
        let joined = char_properties(0x0301, base);
        assert!(joined.should_join());
        assert_eq!(joined.width(), 1);
        let alone = char_properties(0x0301, CharProps::default());
        assert!(!alone.should_join());
        assert!(alone.is_empty());
        let wide = char_properties('日' as u32, CharProps::default());
        assert_eq!(char_properties(0x0301, wide).width(), 2);
    }
}
