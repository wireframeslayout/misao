// 使い方: node scripts/gen-unicode-width.mjs [--check]
// @xterm/headless の既定 (Unicode 6) の文字幅表を、Rust 版の画面モデル (crates/misao-daemon/src/unicode_width_table.rs)
// の入力として書き出す。幅が 1 以外の範囲 (0 = 結合文字・制御文字、2 = 全角) だけを持ち、他は 1。
// --check は書き出さず、コミット済みのファイルが最新かを確かめる (古ければ終了コード 1)。
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import xterm from '@xterm/headless';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const OUT = path.join(root, 'crates/misao-daemon/src/unicode_width_table.rs');
const MAX_CODE_POINT = 0x10ffff;

export function buildTable() {
  const term = new xterm.Terminal({ allowProposedApi: true });
  const unicode = term._core.unicodeService;
  const ranges = [];
  let current;
  for (let cp = 0; cp <= MAX_CODE_POINT; cp++) {
    if (cp >= 0xd800 && cp <= 0xdfff) continue; // サロゲートは char にならない
    const width = unicode.wcwidth(cp);
    if (current && current.width === width && current.end === cp - 1) current.end = cp;
    else {
      if (current && current.width !== 1) ranges.push(current);
      current = { start: cp, end: cp, width };
    }
  }
  if (current && current.width !== 1) ranges.push(current);
  term.dispose();
  const hex = (n) => `0x${n.toString(16).toUpperCase()}`;
  const lines = ranges.map((r) => `    (${hex(r.start)}, ${hex(r.end)}, ${r.width}),`);
  return [
    '// 生成物: node scripts/gen-unicode-width.mjs (手で編集しない)。',
    '// @xterm/headless の既定 (Unicode 6) で幅が 1 以外のコードポイントの範囲 (開始, 終了, 幅)。昇順で重ならない。',
    `pub const NON_SINGLE_WIDTH: [(u32, u32, u8); ${ranges.length}] = [`,
    ...lines,
    '];',
    '',
  ].join('\n');
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const text = buildTable();
  if (process.argv.includes('--check')) {
    if (readFileSync(OUT, 'utf8') !== text) {
      console.error(`${OUT} is out of date; run: node scripts/gen-unicode-width.mjs`);
      process.exit(1);
    }
    console.log(`${OUT} is up to date`);
  } else {
    writeFileSync(OUT, text);
    console.log(`wrote ${OUT}`);
  }
}
