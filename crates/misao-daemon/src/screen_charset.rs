//! 文字集合の切り替え（`ESC ( 0` など）。xterm.js が持つ表と同じ（ASCII の範囲だけを置き換える）。

/// 文字集合の表の識別子（`ESC ( <final>` の final）。`B`（ASCII）は置き換えなしなので持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charset(char);

const GRAPHICS: &[(char, char)] = &[
    ('`', '◆'),
    ('a', '▒'),
    ('b', '␉'),
    ('c', '␌'),
    ('d', '␍'),
    ('e', '␊'),
    ('f', '°'),
    ('g', '±'),
    ('h', '␤'),
    ('i', '␋'),
    ('j', '┘'),
    ('k', '┐'),
    ('l', '┌'),
    ('m', '└'),
    ('n', '┼'),
    ('o', '⎺'),
    ('p', '⎻'),
    ('q', '─'),
    ('r', '⎼'),
    ('s', '⎽'),
    ('t', '├'),
    ('u', '┤'),
    ('v', '┴'),
    ('w', '┬'),
    ('x', '│'),
    ('y', '≤'),
    ('z', '≥'),
    ('{', 'π'),
    ('|', '≠'),
    ('}', '£'),
    ('~', '·'),
];
const UK: &[(char, char)] = &[('#', '£')];
const DUTCH: &[(char, char)] = &[
    ('#', '£'),
    ('@', '¾'),
    ('[', 'i'),
    ('\\', '½'),
    (']', '|'),
    ('{', '¨'),
    ('|', 'f'),
    ('}', '¼'),
    ('~', '´'),
];
const FINNISH: &[(char, char)] = &[
    ('[', 'Ä'),
    ('\\', 'Ö'),
    (']', 'Å'),
    ('^', 'Ü'),
    ('`', 'é'),
    ('{', 'ä'),
    ('|', 'ö'),
    ('}', 'å'),
    ('~', 'ü'),
];
const FRENCH: &[(char, char)] = &[
    ('#', '£'),
    ('@', 'à'),
    ('[', '°'),
    ('\\', 'ç'),
    (']', '§'),
    ('{', 'é'),
    ('|', 'ù'),
    ('}', 'è'),
    ('~', '¨'),
];
const FRENCH_CANADIAN: &[(char, char)] = &[
    ('@', 'à'),
    ('[', 'â'),
    ('\\', 'ç'),
    (']', 'ê'),
    ('^', 'î'),
    ('`', 'ô'),
    ('{', 'é'),
    ('|', 'ù'),
    ('}', 'è'),
    ('~', 'û'),
];
const GERMAN: &[(char, char)] = &[
    ('@', '§'),
    ('[', 'Ä'),
    ('\\', 'Ö'),
    (']', 'Ü'),
    ('{', 'ä'),
    ('|', 'ö'),
    ('}', 'ü'),
    ('~', 'ß'),
];
const ITALIAN: &[(char, char)] = &[
    ('#', '£'),
    ('@', '§'),
    ('[', '°'),
    ('\\', 'ç'),
    (']', 'é'),
    ('`', 'ù'),
    ('{', 'à'),
    ('|', 'ò'),
    ('}', 'è'),
    ('~', 'ì'),
];
const NORWEGIAN: &[(char, char)] = &[
    ('@', 'Ä'),
    ('[', 'Æ'),
    ('\\', 'Ø'),
    (']', 'Å'),
    ('^', 'Ü'),
    ('`', 'ä'),
    ('{', 'æ'),
    ('|', 'ø'),
    ('}', 'å'),
    ('~', 'ü'),
];
const SPANISH: &[(char, char)] = &[
    ('#', '£'),
    ('@', '§'),
    ('[', '¡'),
    ('\\', 'Ñ'),
    (']', '¿'),
    ('{', '°'),
    ('|', 'ñ'),
    ('}', 'ç'),
];
const SWEDISH: &[(char, char)] = &[
    ('@', 'É'),
    ('[', 'Ä'),
    ('\\', 'Ö'),
    (']', 'Å'),
    ('^', 'Ü'),
    ('`', 'é'),
    ('{', 'ä'),
    ('|', 'ö'),
    ('}', 'å'),
    ('~', 'ü'),
];
const SWISS: &[(char, char)] = &[
    ('#', 'ù'),
    ('@', 'à'),
    ('[', 'é'),
    ('\\', 'ç'),
    (']', 'ê'),
    ('^', 'î'),
    ('_', 'è'),
    ('`', 'ô'),
    ('{', 'ä'),
    ('|', 'ö'),
    ('}', 'ü'),
    ('~', 'û'),
];

impl Charset {
    /// `final` に対応する文字集合。`B`（ASCII）と未知の final は None（置き換えなし）。
    pub fn from_final(final_byte: char) -> Option<Self> {
        Self::table_of(final_byte).map(|_| Self(final_byte))
    }

    fn table_of(final_byte: char) -> Option<&'static [(char, char)]> {
        Some(match final_byte {
            '0' => GRAPHICS,
            'A' => UK,
            '4' => DUTCH,
            'C' | '5' => FINNISH,
            'R' => FRENCH,
            'Q' => FRENCH_CANADIAN,
            'K' => GERMAN,
            'Y' => ITALIAN,
            'E' | '6' => NORWEGIAN,
            'Z' => SPANISH,
            'H' | '7' => SWEDISH,
            '=' => SWISS,
            _ => return None,
        })
    }

    /// ASCII の文字を置き換える（`code < 127` のときだけ）。
    pub fn translate(self, code: u32) -> u32 {
        let Some(c) = char::from_u32(code).filter(|_| code < 127) else {
            return code;
        };
        Self::table_of(self.0)
            .and_then(|table| table.iter().find(|&&(from, _)| from == c))
            .map_or(code, |&(_, to)| to as u32)
    }
}

/// 文字集合を指定できる final（xterm.js が登録するもの）。`B` を含む。
pub fn is_charset_final(c: char) -> bool {
    c == 'B' || Charset::table_of(c).is_some()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn graphics_maps_line_drawing_letters_only() {
        let graphics = Charset::from_final('0').unwrap();
        assert_eq!(graphics.translate('q' as u32), '─' as u32);
        assert_eq!(graphics.translate('A' as u32), 'A' as u32);
        assert_eq!(graphics.translate('日' as u32), '日' as u32);
        assert_eq!(Charset::from_final('B'), None);
        assert!(is_charset_final('B'));
        assert!(!is_charset_final('!'));
    }
}
