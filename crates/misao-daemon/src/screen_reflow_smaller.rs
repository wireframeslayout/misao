//! 桁数が減ったときの折り返しの再配置（長い行を折り返して複数の行に分ける。xterm.js の `Buffer._reflowSmaller`）。

use std::collections::HashMap;

use crate::screen_buffer::Buffer;
use crate::screen_line::{Cell, Line};
use crate::screen_reflow::{Group, as_isize, wrapped_trimmed_length};

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

    /// 長すぎる行を `new_cols` 桁で折り返して、複数の行に分ける。
    pub(crate) fn reflow_smaller(&mut self, old_cols: usize, new_cols: usize, new_rows: usize) {
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
