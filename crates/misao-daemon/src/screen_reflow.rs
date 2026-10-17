//! 桁数が変わったときの折り返しの再配置と、`Buffer` のサイズ変更（xterm.js の `Buffer.resize` / `BufferReflow`）。
//! 通常画面だけが再配置する（代替画面は行を切り詰めも伸ばしもしない）。

use std::collections::VecDeque;

use crate::screen_attr::Attr;
use crate::screen_buffer::Buffer;
use crate::screen_line::{Cell, Line};

/// 折り返した行の組 `lines[first..first + count]` の、`index` 番目の行の中身の長さ。
pub(crate) fn wrapped_trimmed_length(group: &[Line], index: usize, cols: usize) -> usize {
    if index == group.len() - 1 {
        return group[index].trimmed_length();
    }
    let has_no_content_at_end = !group[index].has_content(cols - 1) && group[index].width(cols - 1) == 1;
    let ends_in_wide = group[index + 1].width(0) == 2;
    if has_no_content_at_end && ends_in_wide {
        cols - 1
    } else {
        cols
    }
}

pub(crate) fn as_isize(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

/// 折り返し行の組の位置。`first` から `count` 行。
pub(crate) struct Group {
    pub(crate) first: usize,
    pub(crate) count: usize,
}

impl Buffer {
    /// 行数と桁数を変える。桁数が増減したら（通常画面は）折り返しを再配置する。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let old_cols = self.cols;
        self.resize_rows_and_cols(cols, rows);
        if self.has_scrollback && old_cols != cols {
            if cols > old_cols {
                self.reflow_larger(old_cols, cols, rows);
            } else {
                self.reflow_smaller(old_cols, cols, rows);
            }
            if old_cols > cols {
                for line in &mut self.lines {
                    line.resize(cols);
                }
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.ensure_viewport();
    }

    /// 行リストが見えている行ぶんに足りなければ、空行で足す。xterm.js は、ある操作の組み合わせ（空の代替画面を先に resize するなど）で
    /// 行リストと `ybase` が食い違い、画面の外の古い行を読む。そうなった状態を再現せず、空行で補って画面の操作が常に成り立つようにする。
    fn ensure_viewport(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        while self.lines.len() < self.ybase + self.rows {
            let blank = self.blank_line(false, Attr::DEFAULT);
            self.lines.push_back(blank);
        }
    }

    /// 折り返しで始まる行（`is_wrapped`）を前の行にまとめる。
    fn reflow_larger(&mut self, old_cols: usize, new_cols: usize, new_rows: usize) {
        let cursor_abs = self.ybase + self.y;
        let mut to_remove: Vec<(usize, usize)> = Vec::new();
        let mut y = 0;
        while y + 1 < self.lines.len() {
            if !self.lines[y + 1].is_wrapped {
                y += 1;
                continue;
            }
            let mut next = y + 1;
            while next < self.lines.len() && self.lines[next].is_wrapped {
                next += 1;
            }
            let group = Group {
                first: y,
                count: next - y,
            };
            // カーソルのある行はプログラムが直すので触らない。
            if cursor_abs >= group.first && cursor_abs < next {
                y += group.count;
                continue;
            }
            let removed = self.merge_wrapped_group(&group, old_cols, new_cols);
            if removed > 0 {
                to_remove.push((group.first + group.count - removed, removed));
            }
            y += group.count;
        }
        let count_removed: usize = to_remove.iter().map(|&(_, n)| n).sum();
        if count_removed > 0 {
            let mut kept = VecDeque::with_capacity(self.lines.len() - count_removed);
            for (i, line) in std::mem::take(&mut self.lines).into_iter().enumerate() {
                if !to_remove.iter().any(|&(start, n)| i >= start && i < start + n) {
                    kept.push_back(line);
                }
            }
            self.lines = kept;
            for _ in 0..count_removed {
                if self.ybase == 0 {
                    self.y = self.y.saturating_sub(1);
                    if self.lines.len() < new_rows {
                        self.lines.push_back(Line::new(new_cols, false));
                    }
                } else {
                    self.ybase -= 1;
                }
            }
            self.saved.y = self.saved.y.saturating_sub(count_removed);
        }
    }

    /// 折り返しの組のセルを前の行へ詰める。末尾で空になった行の数を返す。
    fn merge_wrapped_group(&mut self, group: &Group, old_cols: usize, new_cols: usize) -> usize {
        let range = group.first..group.first + group.count;
        let mut lines: Vec<Line> = self.lines.range(range.clone()).cloned().collect();
        let (mut dest, mut dest_col) = (0, wrapped_trimmed_length(&lines, 0, old_cols));
        let (mut src, mut src_col) = (1, 0);
        while src < lines.len() {
            let src_len = wrapped_trimmed_length(&lines, src, old_cols);
            let to_copy = (src_len - src_col).min(new_cols - dest_col);
            let cells = lines[src].copy_range(src_col, to_copy);
            lines[dest].paste(dest_col, cells);
            dest_col += to_copy;
            if dest_col == new_cols {
                dest += 1;
                dest_col = 0;
            }
            src_col += to_copy;
            if src_col == src_len {
                src += 1;
                src_col = 0;
            }
            // 直前の行の最後が全角の前半なら、次の行の先頭へ送る
            if dest_col == 0 && dest != 0 && lines[dest - 1].width(new_cols - 1) == 2 {
                let cells = lines[dest - 1].copy_range(new_cols - 1, 1);
                lines[dest].paste(dest_col, cells);
                dest_col += 1;
                lines[dest - 1].set_cell(new_cols - 1, Cell::NULL);
            }
        }
        lines[dest].replace_cells(dest_col, new_cols, Attr::DEFAULT);
        let mut empty = 0;
        for i in (1..lines.len()).rev() {
            if i > dest || lines[i].trimmed_length() == 0 {
                empty += 1;
            } else {
                break;
            }
        }
        for (offset, line) in lines.into_iter().enumerate() {
            self.lines[group.first + offset] = line;
        }
        empty
    }
}
