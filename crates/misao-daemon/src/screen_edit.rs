//! 画面の編集: スクロール、消去、行・文字の挿入と削除（xterm.js の `InputHandler` のうち、セルを書き換える部分）。

use crate::screen_attr::Attr;
use crate::screen_emulator::Emulator;

impl Emulator {
    pub fn scroll_up(&mut self, n: usize) {
        let erase = self.attr.erase();
        let b = self.buf();
        let (top, bottom) = (b.ybase + b.scroll_top, b.ybase + b.scroll_bottom);
        for _ in 0..n.min(bottom - top + 1) {
            b.lines.remove(top);
            let blank = b.blank_line(false, erase);
            b.lines.insert(bottom, blank);
        }
    }

    pub fn scroll_down(&mut self, n: usize) {
        // xterm.js は、`CSI T` だけは消去した行に背景色を引き継がない（既定の属性の空行を入れる）。
        let erase = Attr::DEFAULT;
        let b = self.buf();
        let (top, bottom) = (b.ybase + b.scroll_top, b.ybase + b.scroll_bottom);
        for _ in 0..n.min(bottom - top + 1) {
            b.lines.remove(bottom);
            let blank = b.blank_line(false, erase);
            b.lines.insert(top, blank);
        }
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: Option<usize>) {
        let rows = self.rows;
        let top = top.max(1);
        let bottom = bottom.filter(|&b| b != 0 && b <= rows).unwrap_or(rows);
        if bottom > top {
            let b = self.buf();
            b.scroll_top = top - 1;
            b.scroll_bottom = bottom - 1;
            self.set_cursor(0, 0);
        }
    }

    fn erase_in_row(&mut self, y: usize, start: usize, end: usize, clear_wrap: bool) {
        let erase = self.attr.erase();
        let row = self.buf().row_mut(y);
        row.replace_cells(start, end, erase);
        if clear_wrap {
            row.is_wrapped = false;
        }
    }

    fn reset_row(&mut self, y: usize) {
        let erase = self.attr.erase();
        let row = self.buf().row_mut(y);
        row.fill_null(erase);
        row.is_wrapped = false;
    }

    pub fn erase_in_display(&mut self, mode: usize) {
        self.restrict_cursor(Some(self.cols));
        let (cols, rows) = (self.cols, self.rows);
        let (x, y) = (self.buf_ref().x, self.buf_ref().y);
        match mode {
            0 => {
                self.erase_in_row(y, x, cols, x == 0);
                for row in y + 1..rows {
                    self.reset_row(row);
                }
            }
            1 => {
                self.erase_in_row(y, 0, x + 1, true);
                if x + 1 >= cols
                    && let Some(next) = self.buf().lines.get_mut(y + 1)
                {
                    next.is_wrapped = false;
                }
                for row in 0..y {
                    self.reset_row(row);
                }
            }
            2 => {
                for row in 0..rows {
                    self.reset_row(row);
                }
            }
            3 => {
                let b = self.buf();
                let excess = b.lines.len().saturating_sub(rows);
                if excess > 0 {
                    b.lines.drain(..excess);
                    b.ybase = b.ybase.saturating_sub(excess);
                }
            }
            _ => {}
        }
    }

    pub fn erase_in_line(&mut self, mode: usize) {
        self.restrict_cursor(Some(self.cols));
        let cols = self.cols;
        let (x, y) = (self.buf_ref().x, self.buf_ref().y);
        match mode {
            0 => self.erase_in_row(y, x, cols, x == 0),
            1 => self.erase_in_row(y, 0, x + 1, false),
            2 => self.erase_in_row(y, 0, cols, true),
            _ => {}
        }
    }

    pub fn insert_lines(&mut self, n: usize) {
        self.restrict_cursor(None);
        let erase = self.attr.erase();
        let b = self.buf();
        if b.y > b.scroll_bottom || b.y < b.scroll_top {
            return;
        }
        let (row, bottom) = (b.ybase + b.y, b.ybase + b.scroll_bottom);
        for _ in 0..n.min(bottom - row + 1) {
            b.lines.remove(bottom);
            let blank = b.blank_line(false, erase);
            b.lines.insert(row, blank);
        }
        b.x = 0;
    }

    pub fn delete_lines(&mut self, n: usize) {
        self.restrict_cursor(None);
        let erase = self.attr.erase();
        let b = self.buf();
        if b.y > b.scroll_bottom || b.y < b.scroll_top {
            return;
        }
        let (row, bottom) = (b.ybase + b.y, b.ybase + b.scroll_bottom);
        for _ in 0..n.min(bottom - row + 1) {
            b.lines.remove(row);
            let blank = b.blank_line(false, erase);
            b.lines.insert(bottom, blank);
        }
        b.x = 0;
    }

    pub fn insert_chars(&mut self, n: usize) {
        self.restrict_cursor(None);
        let erase = self.attr.erase();
        let b = self.buf();
        let (x, y) = (b.x, b.y);
        b.row_mut(y).insert_cells(x, n, erase);
    }

    pub fn delete_chars(&mut self, n: usize) {
        self.restrict_cursor(None);
        let erase = self.attr.erase();
        let b = self.buf();
        let (x, y) = (b.x, b.y);
        b.row_mut(y).delete_cells(x, n, erase);
    }

    pub fn erase_chars(&mut self, n: usize) {
        self.restrict_cursor(None);
        let erase = self.attr.erase();
        let b = self.buf();
        let (x, y) = (b.x, b.y);
        b.row_mut(y).replace_cells(x, x + n, erase);
    }

    /// `CSI Ps b`: 直前の文字を繰り返す。
    pub fn repeat_preceding(&mut self, n: usize) {
        if self.preceding.is_empty() {
            return;
        }
        let width = usize::from(self.preceding.width());
        let b = self.buf_ref();
        let Some(start) = b.x.checked_sub(width) else {
            return;
        };
        let text = b.row(b.y).string_at(start);
        let codes: Vec<u32> = text.chars().map(|c| c as u32).collect();
        if codes.is_empty() {
            return;
        }
        // 画面に影響しうる量（全部で画面のセル数のコードポイント）まで。それより多く繰り返しても同じ画面になる。
        let n = n.min(self.rows * self.cols / codes.len());
        let repeated: Vec<u32> = (0..n).flat_map(|_| codes.iter().copied()).collect();
        self.print(&repeated);
    }

    pub fn screen_alignment_pattern(&mut self) {
        self.set_cursor(0, 0);
        let (rows, attr) = (self.rows, self.attr);
        let b = self.buf();
        for t in 0..rows {
            if let Some(line) = b.lines.get_mut(b.ybase + b.y + t) {
                for x in 0..line.len() {
                    line.set_code(x, 'E' as u32, 1, attr);
                }
                line.is_wrapped = false;
            }
        }
        self.set_cursor(0, 0);
    }
}
