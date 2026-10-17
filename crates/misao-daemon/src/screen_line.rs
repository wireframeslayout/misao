//! 画面の 1 行（xterm.js の `BufferLine`）。属性（色など）は持たない: 画面の文字だけが必要で、属性は結果に出ない。
//! 「空のセル」（一度も書かれていない / 消去された）と「空白が書かれたセル」は区別する: 前者だけが行末で取り除かれる。

/// 1 セル。`code` が 0 なら空。全角文字の 2 つ目のセルは、`code` が 0 で幅 0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    code: u32,
    width: u8,
    /// 結合文字を含む文字列（基底の文字 + 結合文字）。
    combined: Option<Box<str>>,
}

impl Cell {
    pub const NULL: Self = Self {
        code: 0,
        width: 1,
        combined: None,
    };

    fn from_code(code: u32, width: u8) -> Self {
        Self {
            code,
            width,
            combined: None,
        }
    }

    /// 文字が入っている（空でない）。
    pub fn has_content(&self) -> bool {
        self.code != 0 || self.combined.is_some()
    }

    fn push_to(&self, out: &mut String) {
        match (&self.combined, self.code) {
            (Some(text), _) => out.push_str(text),
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
        Self {
            cells: vec![Cell::NULL; cols],
            is_wrapped,
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn width(&self, x: usize) -> u8 {
        self.cells[x].width
    }

    pub fn has_content(&self, x: usize) -> bool {
        self.cells[x].has_content()
    }

    pub fn has_width(&self, x: usize) -> bool {
        self.cells[x].width != 0
    }

    pub fn set_null(&mut self, x: usize) {
        self.cells[x] = Cell::NULL;
    }

    pub fn set_code(&mut self, x: usize, code: u32, width: u8) {
        self.cells[x] = Cell::from_code(code, width);
    }

    pub fn set_cell(&mut self, x: usize, cell: Cell) {
        self.cells[x] = cell;
    }

    pub fn cell(&self, x: usize) -> &Cell {
        &self.cells[x]
    }

    /// セルの文字列（空なら空文字列）。
    pub fn string_at(&self, x: usize) -> String {
        let cell = &self.cells[x];
        let mut text = String::new();
        if cell.has_content() {
            cell.push_to(&mut text);
        }
        text
    }

    /// セルに結合文字を足す。空のセルなら、その文字自体を幅 1 で置く。`width` が 0 でなければセルの幅をそれにする。
    pub fn add_code_to_cell(&mut self, x: usize, code: u32, width: u8) {
        let cell = &mut self.cells[x];
        let added = char::from_u32(code).unwrap_or('\u{FFFD}');
        if let Some(text) = cell.combined.take() {
            let mut joined = String::from(text);
            joined.push(added);
            cell.combined = Some(joined.into_boxed_str());
        } else if cell.code != 0 {
            let mut joined = String::new();
            joined.push(char::from_u32(cell.code).unwrap_or('\u{FFFD}'));
            joined.push(added);
            cell.combined = Some(joined.into_boxed_str());
        } else {
            cell.code = code;
            cell.width = 1;
        }
        if width != 0 {
            cell.width = width;
        }
    }

    /// `pos` から `n` セルぶんの空きを作って右へずらす（はみ出したぶんは捨てる）。
    pub fn insert_cells(&mut self, pos: usize, n: usize) {
        let len = self.cells.len();
        let pos = pos % len;
        if pos > 0 && self.width(pos - 1) == 2 {
            self.set_null(pos - 1);
        }
        if n < len - pos {
            for i in (0..len - pos - n).rev() {
                self.cells[pos + n + i] = self.cells[pos + i].clone();
            }
            for i in 0..n {
                self.cells[pos + i] = Cell::NULL;
            }
        } else {
            for i in pos..len {
                self.cells[i] = Cell::NULL;
            }
        }
        if self.width(len - 1) == 2 {
            self.set_null(len - 1);
        }
    }

    /// `pos` から `n` セルを消して左へ詰める（右端は空になる）。
    pub fn delete_cells(&mut self, pos: usize, n: usize) {
        let len = self.cells.len();
        let pos = pos % len;
        if n < len - pos {
            for i in 0..len - pos - n {
                self.cells[pos + i] = self.cells[pos + n + i].clone();
            }
            for i in len - n..len {
                self.cells[i] = Cell::NULL;
            }
        } else {
            for i in pos..len {
                self.cells[i] = Cell::NULL;
            }
        }
        if pos > 0 && self.width(pos - 1) == 2 {
            self.set_null(pos - 1);
        }
        if self.width(pos) == 0 && !self.has_content(pos) {
            self.set_null(pos);
        }
    }

    /// `start..end` を空にする。全角文字を半分だけ残さない。
    pub fn replace_cells(&mut self, start: usize, end: usize) {
        let len = self.cells.len();
        if start > 0 && self.width(start - 1) == 2 {
            self.set_null(start - 1);
        }
        if end < len && end > 0 && self.width(end - 1) == 2 {
            self.set_null(end);
        }
        for x in start..end.min(len) {
            self.cells[x] = Cell::NULL;
        }
    }

    pub fn fill_null(&mut self) {
        self.cells.fill(Cell::NULL);
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
            line.set_code(x, c as u32, width);
            x += 1;
            if width == 2 {
                line.set_code(x, 0, 0);
                x += 1;
            }
        }
        line
    }

    #[test]
    fn explicitly_written_spaces_are_kept_but_empty_cells_are_trimmed() {
        let mut written = line("ab", 6);
        written.set_code(2, ' ' as u32, 1);
        assert_eq!(written.translate(true), "ab ");
        assert_eq!(line("ab", 6).translate(true), "ab");
        assert_eq!(line("a日", 6).translate(true), "a日");
    }

    #[test]
    fn erasing_half_of_a_wide_character_clears_both_halves() {
        let mut l = line("a日b", 6);
        l.replace_cells(2, 3);
        assert_eq!(l.translate(true), "a  b");
    }

    #[test]
    fn insert_and_delete_shift_cells_within_the_line() {
        let mut l = line("abcdef", 6);
        l.insert_cells(1, 2);
        assert_eq!(l.translate(true), "a  bcd");
        l.delete_cells(0, 3);
        assert_eq!(l.translate(true), "bcd");
    }

    #[test]
    fn combining_characters_are_attached_to_the_cell() {
        let mut l = line("e", 4);
        l.add_code_to_cell(0, 0x0301, 1);
        assert_eq!(l.translate(true), "e\u{301}");
        assert_eq!(l.trimmed_length(), 1);
    }
}
