//! 表示幅で列をそろえた表（Node 版の `view/table.ts`）。

const COLUMN_GAP: &str = "  ";

fn char_width(cp: u32) -> usize {
    if matches!(cp, 0x200b | 0x200c | 0x200d | 0xfe0f) || (0x0300..=0x036f).contains(&cp) {
        return 0;
    }
    let is_wide = (0x1100..=0x115f).contains(&cp)
        || (0x2e80..=0xa4cf).contains(&cp)
        || (0xac00..=0xd7a3).contains(&cp)
        || (0xf900..=0xfaff).contains(&cp)
        || (0xfe30..=0xfe6f).contains(&cp)
        || (0xff00..=0xff60).contains(&cp)
        || (0xffe0..=0xffe6).contains(&cp)
        || (0x1f300..=0x1f64f).contains(&cp)
        || (0x1f900..=0x1f9ff).contains(&cp)
        || (0x20000..=0x3fffd).contains(&cp);
    if is_wide { 2 } else { 1 }
}

/// 端末での表示幅。全角・絵文字は 2、結合文字・ゼロ幅は 0。
pub fn display_width(text: &str) -> usize {
    text.chars().map(|c| char_width(c as u32)).sum()
}

/// 表示幅で列をそろえた行にする。最後の列は右側を埋めない。
pub fn render_table(rows: &[Vec<String>]) -> Vec<String> {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..column_count)
        .map(|c| {
            rows.iter()
                .map(|r| r.get(c).map_or(0, |cell| display_width(cell)))
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(c, cell)| {
                    if c + 1 == row.len() {
                        cell.clone()
                    } else {
                        format!(
                            "{cell}{}",
                            " ".repeat(widths[c].saturating_sub(display_width(cell)))
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join(COLUMN_GAP)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|c| (*c).to_string()).collect()
    }

    #[test]
    fn full_width_counts_two() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("表示名"), 6);
        assert_eq!(display_width("(未登録) a"), 10);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn columns_are_aligned_by_display_width_and_the_last_is_not_padded() {
        let lines = render_table(&[row(&["NAME", "LAST"]), row(&["表示名", "1s"]), row(&["ab", "2m"])]);
        assert_eq!(lines, vec!["NAME    LAST", "表示名  1s", "ab      2m"]);
    }

    #[test]
    fn empty_is_empty() {
        assert!(render_table(&[]).is_empty());
    }
}
