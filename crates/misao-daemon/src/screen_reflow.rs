//! 桁数が変わったときの折り返しの再配置と、`Buffer` のサイズ変更（xterm.js の `Buffer.resize` / `BufferReflow`）。
//! 通常画面だけが再配置する（代替画面は行を切り詰めも伸ばしもしない）。

use std::collections::{HashMap, VecDeque};

use crate::screen_buffer::Buffer;
use crate::screen_line::{Cell, Line};

/// 折り返した行の組 `lines[first..first + count]` の、`index` 番目の行の中身の長さ。
fn wrapped_trimmed_length(group: &[Line], index: usize, cols: usize) -> usize {
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

/// 折り返しを `new_cols` 桁に詰め直したときの、各行の長さ。
fn new_line_lengths(group: &[Line], old_cols: usize, new_cols: usize) -> Vec<usize> {
    let mut lengths = Vec::new();
    let needed: usize = (0..group.len())
        .map(|i| wrapped_trimmed_length(group, i, old_cols))
        .sum();
    let (mut src_col, mut src_line, mut available) = (0, 0, 0);
    while available < needed {
        if needed - available < new_cols {
            lengths.push(needed - available);
            break;
        }
        src_col += new_cols;
        let old_trimmed = wrapped_trimmed_length(group, src_line, old_cols);
        if src_col > old_trimmed {
            src_col -= old_trimmed;
            src_line += 1;
        }
        let ends_with_wide = src_col > 0 && group.get(src_line).is_some_and(|l| l.width(src_col - 1) == 2);
        if ends_with_wide {
            src_col -= 1;
        }
        let length = if ends_with_wide { new_cols - 1 } else { new_cols };
        lengths.push(length);
        available += length;
    }
    lengths
}

fn as_isize(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

/// 折り返し行の組の位置。`first` から `count` 行。
struct Group {
    first: usize,
    count: usize,
}

/// 折り返しで増やす行の挿入予定と、ビューポートの調整で行リストから外した行。
///
/// xterm.js の行リストは、末尾の行を外しても中身を残す（長さを減らすだけ）。再配置は外した行も位置で読み続けるので、
/// 同じ結果になるよう、外した行は元の位置で引けるようにしておく。
#[derive(Default)]
struct SmallerState {
    inserts: Vec<(usize, Vec<Line>)>,
    inserted: usize,
    dropped: HashMap<usize, Line>,
}

impl Buffer {
    /// 行リストの `index` 番目（リストから外された行も含む）。
    fn line_or_dropped(&self, index: usize, state: &SmallerState) -> Option<Line> {
        self.lines
            .get(index)
            .or_else(|| state.dropped.get(&index))
            .cloned()
    }

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
        lines[dest].replace_cells(dest_col, new_cols);
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

    /// 長すぎる行を `new_cols` 桁で折り返して、複数の行に分ける。
    fn reflow_smaller(&mut self, old_cols: usize, new_cols: usize, new_rows: usize) {
        let mut state = SmallerState::default();
        let mut y = as_isize(self.lines.len()) - 1;
        while y >= 0 {
            let Ok(last) = usize::try_from(y) else { break };
            let Some(line) = self.line_or_dropped(last, &state) else {
                y -= 1;
                continue;
            };
            if !line.is_wrapped && line.trimmed_length() <= new_cols {
                y -= 1;
                continue;
            }
            let mut first = last;
            while first > 0 && self.line_or_dropped(first, &state).is_some_and(|l| l.is_wrapped) {
                first -= 1;
            }
            let count = last - first + 1;
            // カーソルのある行はプログラムが直すので触らない（カーソルの位置は、ここまでの調整で動いている）。
            let cursor_abs = self.ybase + self.y;
            if cursor_abs < first || cursor_abs >= first + count {
                self.split_wrapped_group(&Group { first, count }, old_cols, new_cols, new_rows, &mut state);
            }
            y = as_isize(first) - 1;
        }
        if !state.inserts.is_empty() {
            self.apply_inserts(state.inserts, state.inserted);
        }
    }

    fn split_wrapped_group(
        &mut self,
        group: &Group,
        old_cols: usize,
        new_cols: usize,
        new_rows: usize,
        state: &mut SmallerState,
    ) {
        let mut lines: Vec<Line> = (group.first..group.first + group.count)
            .filter_map(|i| self.line_or_dropped(i, state))
            .collect();
        let last_len = lines.last().map_or(0, Line::trimmed_length);
        let lengths = new_line_lengths(&lines, old_cols, new_cols);
        // 詰め直したあとの行数の増減。中身のない折り返し行だけの組は、行が減ることがある（負の値）。
        let to_add = as_isize(lengths.len()) - as_isize(group.count);
        let max_len = as_isize(self.max_len);
        let trimmed = if self.ybase == 0 && self.y != self.lines.len() - 1 {
            (as_isize(self.y) - max_len + to_add).max(0)
        } else {
            (as_isize(self.lines.len()) - max_len + to_add).max(0)
        };
        let added = usize::try_from(to_add).unwrap_or(0);
        lines.extend((0..added).map(|_| Line::new(new_cols, true)));
        let length_at = |i: isize| usize::try_from(i).ok().and_then(|i| lengths.get(i).copied());
        let mut dest = as_isize(lengths.len()) - 1;
        let mut dest_size = length_at(dest);
        if dest_size == Some(0) {
            dest -= 1;
            dest_size = length_at(dest);
        }
        let mut src = as_isize(lines.len() - added) - 1;
        let mut src_col = last_len;
        while src >= 0 {
            let (Some(d), Some(size)) = (usize::try_from(dest).ok().filter(|&d| d < lines.len()), dest_size)
            else {
                break;
            };
            let s = usize::try_from(src).unwrap_or(0);
            let to_copy = src_col.min(size);
            let cells = lines[s].copy_range(src_col - to_copy, to_copy);
            lines[d].paste(size - to_copy, cells);
            let remaining = size - to_copy;
            dest_size = Some(remaining);
            if remaining == 0 {
                dest -= 1;
                dest_size = length_at(dest);
            }
            src_col -= to_copy;
            if src_col == 0 {
                src -= 1;
                let wrapped_index = usize::try_from(src.max(0)).unwrap_or(0);
                src_col = wrapped_trimmed_length(&lines, wrapped_index, self.cols);
            }
        }
        for (line, &length) in lines.iter_mut().zip(&lengths) {
            if length < new_cols {
                line.set_cell(length, Cell::NULL);
            }
        }
        let tail = lines.split_off(group.count.min(lines.len()));
        for (offset, line) in lines.into_iter().enumerate() {
            let index = group.first + offset;
            match self.lines.get_mut(index) {
                Some(slot) => *slot = line,
                None => {
                    state.dropped.insert(index, line);
                }
            }
        }
        if !tail.is_empty() {
            state
                .inserts
                .push((group.first + group.count + state.inserted, tail));
            state.inserted += added;
        }
        let mut adjustments = to_add - trimmed;
        while adjustments > 0 {
            adjustments -= 1;
            if self.ybase == 0 {
                if self.y < new_rows - 1 {
                    self.y += 1;
                    if let Some(line) = self.lines.pop_back() {
                        state.dropped.insert(self.lines.len(), line);
                    }
                } else {
                    self.ybase += 1;
                }
            } else if self.ybase
                < self
                    .max_len
                    .min(self.lines.len() + state.inserted)
                    .saturating_sub(new_rows)
            {
                self.ybase += 1;
            }
        }
        let saved = (as_isize(self.saved.y) + to_add).min(as_isize(self.ybase + new_rows) - 1);
        self.saved.y = usize::try_from(saved).unwrap_or(0);
    }

    /// 折り返しで増えた行を、行リストの正しい位置に挿入する（古い行が上限を超えたら捨てる）。
    fn apply_inserts(&mut self, inserts: Vec<(usize, Vec<Line>)>, count: usize) {
        let original: Vec<Line> = std::mem::take(&mut self.lines).into_iter().collect();
        let original_len = original.len();
        let new_len = (original_len + count).min(self.max_len);
        let mut slots: Vec<Option<Line>> = (0..new_len).map(|_| None).collect();
        let mut original: Vec<Option<Line>> = original.into_iter().map(Some).collect();
        let mut original_index = isize::try_from(original_len).unwrap_or(0) - 1;
        let mut pending = inserts.into_iter();
        let mut next = pending.next();
        let mut inserted_so_far = 0;
        let mut i = isize::try_from((self.max_len - 1).min(original_len + count - 1)).unwrap_or(0);
        while i >= 0 {
            let start_ok = next.as_ref().is_some_and(|(start, _)| {
                isize::try_from(*start).unwrap_or(0)
                    > original_index + isize::try_from(inserted_so_far).unwrap_or(0)
            });
            if start_ok && let Some((_, new_lines)) = next.take() {
                inserted_so_far += new_lines.len();
                for line in new_lines.into_iter().rev() {
                    if let Some(slot) = usize::try_from(i).ok().and_then(|u| slots.get_mut(u)) {
                        *slot = Some(line);
                    }
                    i -= 1;
                }
                i += 1;
                next = pending.next();
            } else if let Some(line) = usize::try_from(original_index)
                .ok()
                .and_then(|o| original[o].take())
            {
                if let Some(slot) = usize::try_from(i).ok().and_then(|u| slots.get_mut(u)) {
                    *slot = Some(line);
                }
                original_index -= 1;
            } else {
                original_index -= 1;
            }
            i -= 1;
        }
        self.lines = slots.into_iter().flatten().collect();
    }
}
