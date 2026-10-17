import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

// Rust 版の差分テスト (crates/misao-daemon) の入力と期待値は、この Node 版から作った生成物。
// Node 版の振る舞い (xterm の更新、判定の定数、前面プロセスの解釈) を変えたら、再生成してコミットする。
// 生成スクリプトは dist (要 `npm run build`) を使う。ビルド前は確かめられないので、飛ばす (CI はビルドしてから動かす)。

const SCRIPTS = new URL('../../../scripts/', import.meta.url);
const built = existsSync(new URL('../dist/state-tracker.js', import.meta.url));
const skip = built ? false : 'run `npm run build` first (the generator scripts use packages/daemon/dist)';

test('crates/misao-daemon/tests/fixtures/node-diff は Node 版の出力と一致する', { skip }, async () => {
  const { buildFixtures, DEFAULT_DIR } = await import(new URL('gen-rust-diff-fixtures.mjs', SCRIPTS).href);
  const expected: Map<string, string> = await buildFixtures();
  const onDisk = readdirSync(DEFAULT_DIR).sort();
  assert.deepEqual(onDisk, [...expected.keys()].sort(), 'fixture files differ; run: node scripts/gen-rust-diff-fixtures.mjs');
  for (const [name, text] of expected) {
    assert.equal(readFileSync(path.join(DEFAULT_DIR, name), 'utf8'), text, `${name} is out of date; run: node scripts/gen-rust-diff-fixtures.mjs`);
  }
});

test('crates/misao-daemon/src/unicode_width_table.rs は @xterm/headless の文字幅表と一致する', async () => {
  const { buildTable, OUT } = await import(new URL('gen-unicode-width.mjs', SCRIPTS).href);
  assert.equal(readFileSync(OUT, 'utf8'), buildTable(), 'out of date; run: node scripts/gen-unicode-width.mjs');
});
