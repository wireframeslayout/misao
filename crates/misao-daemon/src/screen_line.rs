//! 画面の 1 行（xterm.js の `BufferLine`）。セルは文字と属性（色・飾り）を持つ。属性は `pane.screen` の文字には出ず、attach の snapshot だけが使う。
//! 「空のセル」（一度も書かれていない / 消去された）と「空白が書かれたセル」は区別する: 前者だけが行末で取り除かれる。

use crate::screen_attr::Attr;

/// 1 つのセルに付けられる結合文字の数の上限（基底の文字は数えない）。xterm.js には上限が無い。
const MAX_COMBINING: usize = 32;

/// 基底の文字 + 結合文字の文字列。セルを小さく保つため、まれな結合文字つきのセルだけが箱に入れて持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Combined(String);

/// 1 セル。`code` が 0 なら空。全角文字の 2 つ目のセルは、`code` が 0 で幅 0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    code: u32,
    width: u8,
    attr: Attr,
    /// 結合文字を含む文字列（基底の文字 + 結合文字）。
    combined: Option<Box<Combined>>,
}

impl Cell {
    pub const NULL: Self = Self {
        code: 0,
        width: 1,
        attr: Attr::DEFAULT,
        combined: None,
    };

    /// 空のセル（属性だけを持つ）。
    pub fn null(attr: Attr) -> Self {
        Self { attr, ..Self::NULL }
    }

    fn from_code(code: u32, width: u8, attr: Attr) -> Self {
        Self {
            code,
            width,
            attr,
            combined: None,
        }
    }

    pub fn attr(&self) -> Attr {
        self.attr
    }

    pub fn width(&self) -> u8 {
        self.width
    }

    /// 文字が入っている（空でない）。
    pub fn has_content(&self) -> bool {
        self.code != 0 || self.combined.is_some()
    }

    fn push_to(&self, out: &mut String) {
        match (&self.combined, self.code) {
            (Some(text), _) => out.push_str(&text.0),
            (None, 0) => out.push(' '),
            (None, code) => out.push(char::from_u32(code).unwrap_or('\u{FFFD}')),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Line {
    cells: Vec<Cell>,
    /// 前の行から折り返して続いている行か。
    pub is_wrapped: bool,
}

impl Line {
    pub fn new(cols: usize, is_wrapped: bool) -> Self {
        Self::filled(cols, is_wrapped, Attr::DEFAULT)
    }

    /// 全セルが空で、属性が `attr` の行。
    pub fn filled(cols: usize, is_wrapped: bool, attr: Attr) -> Self {
        Self {
            cells: vec![Cell::null(attr); cols],
            is_wrapped,
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    // 行の外（折り返し待ちで桁数の位置にいるときなど）への書き込みは無視し、読み出しは空のセルとして扱う
    // （xterm.js は型付き配列の範囲外を同じように扱う）。

    pub fn width(&self, x: usize) -> u8 {
        self.cell(x).width
    }

    pub fn has_content(&self, x: usize) -> bool {
        self.cell(x).has_content()
    }

    pub fn has_width(&self, x: usize) -> bool {
        self.cell(x).width != 0
    }

    pub fn set_null(&mut self, x: usize, attr: Attr) {
        self.set_cell(x, Cell::null(attr));
    }

    pub fn set_code(&mut self, x: usize, code: u32, width: u8, attr: Attr) {
        self.set_cell(x, Cell::from_code(code, width, attr));
    }

    pub fn set_cell(&mut self, x: usize, cell: Cell) {
        if let Some(slot) = self.cells.get_mut(x) {
            *slot = cell;
        }
    }

    pub fn cell(&self, x: usize) -> &Cell {
        static EMPTY: Cell = Cell::NULL;
        self.cells.get(x).unwrap_or(&EMPTY)
    }

    /// セルの文字列（空なら空文字列）。
    pub fn string_at(&self, x: usize) -> String {
        let cell = self.cell(x);
        let mut text = String::new();
        if cell.has_content() {
            cell.push_to(&mut text);
        }
        text
    }

    /// セルに結合文字を足す。空のセルなら、その文字自体を幅 1 で置く。`width` が 0 でなければセルの幅をそれにする。
    /// 1 つのセルに付けられる結合文字は `MAX_COMBINING` まで（超えたぶんは捨てる）。長い結合列で行の更新が遅くならないように。
    pub fn add_code_to_cell(&mut self, x: usize, code: u32, width: u8) {
        let Some(cell) = self.cells.get_mut(x) else {
            return;
        };
        let added = char::from_u32(code).unwrap_or('\u{FFFD}');
        if let Some(combined) = cell.combined.as_mut() {
            if combined.0.chars().count() <= MAX_COMBINING {
                combined.0.push(added);
            }
        } else if cell.code != 0 {
            let mut joined = String::new();
            joined.push(char::from_u32(cell.code).unwrap_or('\u{FFFD}'));
            joined.push(added);
            cell.combined = Some(Box::new(Combined(joined)));
        } else {
            cell.code = code;
            cell.width = 1;
        }
        if width != 0 {
            cell.width = width;
        }
    }

    /// `pos` から `n` セルぶんの空きを作って右へずらす（はみ出したぶんは捨てる）。
    pub fn insert_cells(&mut self, pos: usize, n: usize, fill: Attr) {
        let len = self.cells.len();
        let pos = pos % len;
        if pos > 0 && self.width(pos - 1) == 2 {
            self.set_null(pos - 1, fill);
        }
        if n < len - pos {
            for i in (0..len - pos - n).rev() {
                self.cells[pos + n + i] = self.cells[pos + i].clone();
            }
            for i in 0..n {
                self.cells[pos + i] = Cell::null(fill);
            }
        } else {
            for i in pos..len {
                self.cells[i] = Cell::null(fill);
            }
        }
        if self.width(len - 1) == 2 {
            self.set_null(len - 1, fill);
        }
    }

    /// `pos` から `n` セルを消して左へ詰める（右端は空になる）。
    pub fn delete_cells(&mut self, pos: usize, n: usize, fill: Attr) {
        let len = self.cells.len();
        let pos = pos % len;
        if n < len - pos {
            for i in 0..len - pos - n {
                self.cells[pos + i] = self.cells[pos + n + i].clone();
            }
            for i in len - n..len {
                self.cells[i] = Cell::null(fill);
            }
        } else {
            for i in pos..len {
                self.cells[i] = Cell::null(fill);
            }
        }
        if pos > 0 && self.width(pos - 1) == 2 {
            self.set_null(pos - 1, fill);
        }
        if self.width(pos) == 0 && !self.has_content(pos) {
            self.set_null(pos, fill);
        }
    }

    /// `start..end` を空にする。全角文字を半分だけ残さない。
    pub fn replace_cells(&mut self, start: usize, end: usize, fill: Attr) {
        let len = self.cells.len();
        if start > 0 && self.width(start - 1) == 2 {
            self.set_null(start - 1, fill);
        }
        if end < len && end > 0 && self.width(end - 1) == 2 {
            self.set_null(end, fill);
        }
        for x in start..end.min(len) {
            self.cells[x] = Cell::null(fill);
        }
    }

    pub fn fill_null(&mut self, fill: Attr) {
        self.cells.fill(Cell::null(fill));
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::NULL);
    }

    /// 最後の文字のセルの右端（文字が無ければ 0）。
    pub fn trimmed_length(&self) -> usize {
        self.cells
            .iter()
            .rposition(Cell::has_content)
            .map_or(0, |i| i + usize::from(self.cells[i].width))
    }

    /// `translateToString`。`trim_right` なら行末の空のセルを除く。
    pub fn translate(&self, trim_right: bool) -> String {
        let end = if trim_right {
            self.trimmed_length()
        } else {
            self.cells.len()
        };
        let mut text = String::new();
        let mut x = 0;
        while x < end {
            let cell = &self.cells[x];
            cell.push_to(&mut text);
            x += usize::from(cell.width.max(1));
        }
        text
    }

    /// `start` から `len` セルを取り出す（折り返しの再配置で、行の間や行の中でセルを移すのに使う）。
    pub fn copy_range(&self, start: usize, len: usize) -> Vec<Cell> {
        self.cells[start..start + len].to_vec()
    }

    /// `dst` からセルを書き込む。
    pub fn paste(&mut self, dst: usize, cells: Vec<Cell>) {
        for (i, cell) in cells.into_iter().enumerate() {
            self.cells[dst + i] = cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, cols: usize) -> Line {
        let mut line = Line::new(cols, false);
        let mut x = 0;
        for c in text.chars() {
            let width = if c == '日' { 2 } else { 1 };
            line.set_code(x, c as u32, width, Attr::DEFAULT);
            x += 1;
            if width == 2 {
                line.set_code(x, 0, 0, Attr::DEFAULT);
                x += 1;
            }
        }
        line
    }

    #[test]
    fn explicitly_written_spaces_are_kept_but_empty_cells_are_trimmed() {
        let mut written = line("ab", 6);
        written.set_code(2, ' ' as u32, 1, Attr::DEFAULT);
        assert_eq!(written.translate(true), "ab ");
        assert_eq!(line("ab", 6).translate(true), "ab");
        assert_eq!(line("a日", 6).translate(true), "a日");
    }

    #[test]
    fn erasing_half_of_a_wide_character_clears_both_halves() {
        let mut l = line("a日b", 6);
        l.replace_cells(2, 3, Attr::DEFAULT);
        assert_eq!(l.translate(true), "a  b");
    }

    #[test]
    fn insert_and_delete_shift_cells_within_the_line() {
        let mut l = line("abcdef", 6);
        l.insert_cells(1, 2, Attr::DEFAULT);
        assert_eq!(l.translate(true), "a  bcd");
        l.delete_cells(0, 3, Attr::DEFAULT);
        assert_eq!(l.translate(true), "bcd");
    }

    #[test]
    fn a_very_long_run_of_combining_marks_is_capped_per_cell() {
        let mut l = line("e", 4);
        for _ in 0..200_000 {
            l.add_code_to_cell(0, 0x0301, 1);
        }
        assert_eq!(l.translate(true).chars().count(), 1 + MAX_COMBINING);
    }

    #[test]
    fn combining_characters_are_attached_to_the_cell() {
        let mut l = line("e", 4);
        l.add_code_to_cell(0, 0x0301, 1);
        assert_eq!(l.translate(true), "e\u{301}");
        assert_eq!(l.trimmed_length(), 1);
    }
}
