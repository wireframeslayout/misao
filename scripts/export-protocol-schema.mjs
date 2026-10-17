// 使い方: node scripts/export-protocol-schema.mjs [--check] [--out PATH]
// @misao/protocol の Zod スキーマを JSON Schema に書き出す (Rust 側の型との適合テストの入力)。
// --check は書き出さず、既存ファイルが最新かを確かめる (古ければ終了コード 1)。要 `npm run build`。
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildProtocolJsonSchema } from '@misao/protocol';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_OUT = path.join(root, 'crates/misao-protocol/schema/protocol.json');

const args = process.argv.slice(2);
const check = args.includes('--check');
const outIndex = args.indexOf('--out');
const out = outIndex >= 0 ? path.resolve(args[outIndex + 1] ?? '') : DEFAULT_OUT;
if (outIndex >= 0 && args[outIndex + 1] === undefined) throw new Error('--out requires a path');

const text = `${JSON.stringify(buildProtocolJsonSchema(), null, 2)}\n`;
if (check) {
  if (readFileSync(out, 'utf8') !== text) {
    console.error(`${out} is out of date; run: node scripts/export-protocol-schema.mjs`);
    process.exit(1);
  }
  console.log(`${out} is up to date`);
} else {
  writeFileSync(out, text);
  console.log(`wrote ${out}`);
}
