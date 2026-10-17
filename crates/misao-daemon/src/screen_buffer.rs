//! 画面のバッファ 1 つ分（xterm.js の `Buffer`）。通常画面は直近 `scrollback` 行を、代替画面は見えている行だけを持つ。
//! 行は古い順に並べ、`ybase` 番目から `rows` 行が見えている行。

use std::collections::{BTreeSet, VecDeque};

use crate::screen_charset::Charset;
use crate::screen_line::Line;

const TAB_STOP_WIDTH: usize = 8;

/// `ESC 7` / `CSI s` で保存するカーソル。`y` は行リスト内の位置（スクロールしても同じ行を指す）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SavedCursor {
    pub x: usize,
    pub y: usize,
    pub charset: Option<Charset>,
}

pub struct Buffer {
    pub lines: VecDeque<Line>,
    pub(crate) max_len: usize,
    scrollback: usize,
    pub(crate) has_scrollback: bool,
    pub cols: usize,
    pub rows: usize,
    pub ybase: usize,
    pub x: usize,
    pub y: usize,
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    tabs: BTreeSet<usize>,
    pub saved: SavedCursor,
}

impl Buffer {
    pub fn new(cols: usize, rows: usize, scrollback: usize, has_scrollback: bool) -> Self {
        let mut buffer = Self {
            lines: VecDeque::new(),
            max_len: Self::max_len_for(rows, scrollback, has_scrollback),
            scrollback,
            has_scrollback,
            cols,
            rows,
            ybase: 0,
            x: 0,
            y: 0,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tabs: BTreeSet::new(),
            saved: SavedCursor::default(),
        };
        buffer.setup_tab_stops(None);
        buffer
    }

    fn max_len_for(rows: usize, scrollback: usize, has_scrollback: bool) -> usize {
        if has_scrollback { rows + scrollback } else { rows }
    }

    pub fn blank_line(&self, is_wrapped: bool) -> Line {
        Line::new(self.cols, is_wrapped)
    }

    /// 行が無ければ、見えている行ぶんの空行を作る。
    pub fn fill_viewport_rows(&mut self) {
        if self.lines.is_empty() {
            for _ in 0..self.rows {
                self.lines.push_back(self.blank_line(false));
            }
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.max_len = Self::max_len_for(self.rows, self.scrollback, self.has_scrollback);
        self.ybase = 0;
        self.x = 0;
        self.y = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.setup_tab_stops(None);
    }

    /// 見えている `y` 行目（0 始まり）。
    pub fn row_mut(&mut self, y: usize) -> &mut Line {
        let index = self.ybase + y;
        &mut self.lines[index]
    }

    pub fn row(&self, y: usize) -> &Line {
        &self.lines[self.ybase + y]
    }

    /// 行リストの `index` に挿入する。上限を超えたぶんは古い行から捨てる。
    pub fn splice_in(&mut self, index: usize, line: Line) {
        self.lines.insert(index, line);
        while self.lines.len() > self.max_len {
            self.lines.pop_front();
        }
    }

    /// 画面を 1 行スクロールする（`BufferService.scroll`）。スクロール領域の先頭が画面の先頭なら、消える行はスクロールバックに入る。
    pub fn scroll(&mut self, is_wrapped: bool) {
        let new_line = self.blank_line(is_wrapped);
        let top = self.ybase + self.scroll_top;
        let bottom = self.ybase + self.scroll_bottom;
        if self.scroll_top == 0 {
            let is_full = self.lines.len() >= self.max_len;
            if bottom == self.lines.len() - 1 {
                self.lines.push_back(new_line);
                if is_full {
                    self.lines.pop_front();
                }
            } else {
                self.splice_in(bottom + 1, new_line);
            }
            if !is_full {
                self.ybase += 1;
            }
        } else {
            self.lines.remove(top);
            self.lines.insert(bottom, new_line);
        }
    }

    /// 次のタブ位置（右端を超えれば右端）。
    pub fn next_stop(&self, from: usize) -> usize {
        let mut x = from;
        loop {
            x += 1;
            if self.tabs.contains(&x) || x >= self.cols {
                break;
            }
        }
        x.min(self.cols - 1)
    }

    /// 前のタブ位置（左端を下回れば左端）。
    pub fn prev_stop(&self, from: usize) -> usize {
        let mut x = isize::try_from(from).unwrap_or(isize::MAX);
        loop {
            x -= 1;
            let stop = usize::try_from(x).is_ok_and(|u| self.tabs.contains(&u));
            if stop || x <= 0 {
                break;
            }
        }
        usize::try_from(x.max(0)).unwrap_or(0).min(self.cols - 1)
    }

    pub fn set_tab_stop(&mut self, x: usize) {
        self.tabs.insert(x);
    }

    pub fn clear_tab_stop(&mut self, x: usize) {
        self.tabs.remove(&x);
    }

    pub fn clear_all_tab_stops(&mut self) {
        self.tabs.clear();
    }

    /// `from` が None なら全部作り直す。Some なら、そこから右（無ければ直前の位置から）に足す。
    pub fn setup_tab_stops(&mut self, from: Option<usize>) {
        let mut x = match from {
            Some(x) if self.tabs.contains(&x) => x,
            Some(x) => self.prev_stop(x),
            None => {
                self.tabs.clear();
                0
            }
        };
        while x < self.cols {
            self.tabs.insert(x);
            x += TAB_STOP_WIDTH;
        }
    }

    /// 行数と桁数を変える（`Buffer.resize` の、折り返しの再配置を除いた部分）。折り返しは `screen_reflow` が行う。
    pub(crate) fn resize_rows_and_cols(&mut self, cols: usize, rows: usize) -> usize {
        let new_max = Self::max_len_for(rows, self.scrollback, self.has_scrollback);
        if new_max > self.max_len {
            self.max_len = new_max;
        }
        let mut y_shift = 0;
        if !self.lines.is_empty() {
            if self.cols < cols {
                for line in &mut self.lines {
                    line.resize(cols);
                }
            }
            if self.rows < rows {
                for _ in self.rows..rows {
                    if self.lines.len() < rows + self.ybase {
                        if self.ybase > 0 && self.lines.len() <= self.ybase + self.y + y_shift + 1 {
                            self.ybase -= 1;
                            y_shift += 1;
                        } else {
                            self.lines.push_back(Line::new(cols, false));
                        }
                    }
                }
            } else {
                for _ in rows..self.rows {
                    if self.lines.len() > rows + self.ybase {
                        if self.lines.len() > self.ybase + self.y + 1 {
                            self.lines.pop_back();
                        } else {
                            self.ybase += 1;
                        }
                    }
                }
            }
            if new_max < self.max_len {
                let excess = self.lines.len().saturating_sub(new_max);
                if excess > 0 {
                    self.lines.drain(..excess);
                    self.ybase = self.ybase.saturating_sub(excess);
                    self.saved.y = self.saved.y.saturating_sub(excess);
                }
                self.max_len = new_max;
            }
            self.x = self.x.min(cols - 1);
            self.y = self.y.min(rows - 1);
            self.y += y_shift;
            self.saved.x = self.saved.x.min(cols - 1);
            self.scroll_top = 0;
        }
        self.scroll_bottom = rows - 1;
        y_shift
    }
}
