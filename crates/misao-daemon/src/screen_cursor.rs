//! カーソルの移動、タブ、改行（xterm.js の `InputHandler` のうち、カーソルを動かす部分）。

use crate::screen_emulator::Emulator;

fn as_isize(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

impl Emulator {
    pub(crate) fn restrict_cursor(&mut self, max_col: Option<usize>) {
        let (cols, rows, origin) = (self.cols, self.rows, self.origin);
        let b = self.buf();
        b.x = b.x.min(max_col.unwrap_or(cols - 1));
        b.y = if origin {
            b.y.clamp(b.scroll_top, b.scroll_bottom)
        } else {
            b.y.min(rows - 1)
        };
    }

    pub(crate) fn set_cursor(&mut self, x: isize, y: isize) {
        let origin = self.origin;
        let b = self.buf();
        b.x = usize::try_from(x).unwrap_or(0);
        let top = if origin {
            isize::try_from(b.scroll_top).unwrap_or(0)
        } else {
            0
        };
        b.y = usize::try_from(top + y).unwrap_or(0);
        self.restrict_cursor(None);
    }

    pub(crate) fn move_cursor(&mut self, dx: isize, dy: isize) {
        self.restrict_cursor(None);
        let b = self.buf_ref();
        let (x, y) = (as_isize(b.x), as_isize(b.y));
        self.set_cursor(x + dx, y + dy);
    }

    pub fn cursor_up(&mut self, n: usize) {
        let b = self.buf_ref();
        let diff = as_isize(b.y) - as_isize(b.scroll_top);
        let n = as_isize(n);
        self.move_cursor(0, -if diff >= 0 { diff.min(n) } else { n });
    }

    pub fn cursor_down(&mut self, n: usize) {
        let b = self.buf_ref();
        let diff = as_isize(b.scroll_bottom) - as_isize(b.y);
        let n = as_isize(n);
        self.move_cursor(0, if diff >= 0 { diff.min(n) } else { n });
    }

    pub fn cursor_forward(&mut self, n: usize) {
        self.move_cursor(as_isize(n), 0);
    }

    pub fn cursor_backward(&mut self, n: usize) {
        self.move_cursor(-as_isize(n), 0);
    }

    pub fn cursor_next_line(&mut self, n: usize) {
        self.cursor_down(n);
        self.buf().x = 0;
    }

    pub fn cursor_preceding_line(&mut self, n: usize) {
        self.cursor_up(n);
        self.buf().x = 0;
    }

    pub fn cursor_column(&mut self, col: usize) {
        let y = as_isize(self.buf_ref().y);
        self.set_cursor(as_isize(col) - 1, y);
    }

    pub fn cursor_line(&mut self, row: usize) {
        let x = as_isize(self.buf_ref().x);
        self.set_cursor(x, as_isize(row) - 1);
    }

    pub fn cursor_position(&mut self, row: usize, col: usize) {
        self.set_cursor(as_isize(col) - 1, as_isize(row) - 1);
    }

    pub fn carriage_return(&mut self) {
        self.buf().x = 0;
    }

    pub fn backspace(&mut self) {
        if !self.reverse_wraparound {
            self.restrict_cursor(None);
            let b = self.buf();
            b.x = b.x.saturating_sub(1);
            return;
        }
        self.restrict_cursor(Some(self.cols));
        let cols = self.cols;
        let b = self.buf();
        if b.x > 0 {
            b.x -= 1;
        } else if b.y > b.scroll_top && b.y <= b.scroll_bottom && b.row(b.y).is_wrapped {
            let y = b.y;
            b.row_mut(y).is_wrapped = false;
            b.y -= 1;
            b.x = cols - 1;
            let row = b.row(b.y);
            if row.has_width(b.x) && !row.has_content(b.x) {
                b.x -= 1;
            }
        }
        self.restrict_cursor(None);
    }

    pub fn tab(&mut self) {
        let cols = self.cols;
        let b = self.buf();
        if b.x < cols {
            b.x = b.next_stop(b.x);
        }
    }

    pub fn cursor_forward_tab(&mut self, n: usize) {
        let cols = self.cols;
        let b = self.buf();
        if b.x < cols {
            for _ in 0..n.min(cols) {
                b.x = b.next_stop(b.x);
            }
        }
    }

    pub fn cursor_backward_tab(&mut self, n: usize) {
        let cols = self.cols;
        let b = self.buf();
        if b.x < cols {
            for _ in 0..n.min(cols) {
                b.x = b.prev_stop(b.x);
            }
        }
    }

    pub fn tab_set(&mut self) {
        let b = self.buf();
        b.set_tab_stop(b.x);
    }

    pub fn tab_clear(&mut self, mode: usize) {
        let b = self.buf();
        match mode {
            0 => b.clear_tab_stop(b.x),
            3 => b.clear_all_tab_stops(),
            _ => {}
        }
    }

    pub fn line_feed(&mut self) {
        let (rows, cols, convert_eol) = (self.rows, self.cols, self.convert_eol);
        let b = self.buf();
        if convert_eol {
            b.x = 0;
        }
        b.y += 1;
        if b.y == b.scroll_bottom + 1 {
            b.y -= 1;
            b.scroll(false);
        } else if b.y >= rows {
            b.y = rows - 1;
        } else {
            let y = b.y;
            b.row_mut(y).is_wrapped = false;
        }
        if b.x >= cols {
            b.x -= 1;
        }
    }

    pub fn index(&mut self) {
        self.restrict_cursor(None);
        let rows = self.rows;
        let b = self.buf();
        b.y += 1;
        if b.y == b.scroll_bottom + 1 {
            b.y -= 1;
            b.scroll(false);
        } else if b.y >= rows {
            b.y = rows - 1;
        }
        self.restrict_cursor(None);
    }

    pub fn next_line(&mut self) {
        self.buf().x = 0;
        self.index();
    }

    pub fn reverse_index(&mut self) {
        self.restrict_cursor(None);
        let b = self.buf();
        if b.y == b.scroll_top {
            let (top, bottom) = (b.ybase + b.scroll_top, b.ybase + b.scroll_bottom);
            b.lines.remove(bottom);
            let blank = b.blank_line(false);
            b.lines.insert(top, blank);
        } else {
            b.y = b.y.saturating_sub(1);
            self.restrict_cursor(None);
        }
    }

    /// `CSI Ps e`
    pub fn vertical_relative(&mut self, n: usize) {
        self.move_cursor(0, as_isize(n));
    }
}
