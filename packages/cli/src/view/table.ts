const COLUMN_GAP = '  ';

/** 端末での表示幅。全角・絵文字は 2、結合文字・ゼロ幅は 0。 */
export function displayWidth(text: string): number {
  let width = 0;
  for (const ch of text) width += charWidth(ch.codePointAt(0)!);
  return width;
}

function charWidth(cp: number): number {
  if (cp === 0x200b || cp === 0x200c || cp === 0x200d || cp === 0xfe0f) return 0;
  if (cp >= 0x0300 && cp <= 0x036f) return 0;
  const isWide =
    (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0xa4cf) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) ||
    (cp >= 0x1f900 && cp <= 0x1f9ff) ||
    (cp >= 0x20000 && cp <= 0x3fffd);
  return isWide ? 2 : 1;
}

/** 表示幅で列をそろえた行にする。最後の列は右側を埋めない。 */
export function renderTable(rows: readonly (readonly string[])[]): string[] {
  const columnCount = Math.max(0, ...rows.map((r) => r.length));
  const widths = Array.from({ length: columnCount }, (_, c) => Math.max(...rows.map((r) => displayWidth(r[c] ?? ''))));
  return rows.map((row) =>
    row
      .map((cell, c) => (c === row.length - 1 ? cell : cell + ' '.repeat(widths[c]! - displayWidth(cell))))
      .join(COLUMN_GAP),
  );
}
