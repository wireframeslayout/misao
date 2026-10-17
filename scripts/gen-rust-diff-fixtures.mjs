// 使い方: node scripts/gen-rust-diff-fixtures.mjs [--check]
// Rust 版の差分テスト (crates/misao-daemon/src/node_diff_tests.rs) の入力と期待値を、Node 版で作る。
// 入力は scripts/rust-diff/scenarios.mjs、期待値は Node 版 (xterm + StateTracker + foreground) の出力。
// --check は書き出さず、コミット済みのファイルが最新かを確かめる (古ければ終了コード 1)。要 `npm run build`。
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { commandNameFromArgv, parseForegroundPgid, sanitizeCommandName } from '../packages/daemon/dist/foreground.js';
import { runScenario } from './rust-diff/node-reference.mjs';
import { foregroundArgv, foregroundStats, fuzzScenarios, scenarios } from './rust-diff/scenarios.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const optionValue = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
// --out DIR: 別の場所へ書く。--extra-fuzz N: コミットしない追加の乱数シナリオ (調査用)。
export const DEFAULT_DIR = path.join(root, 'crates/misao-daemon/tests/fixtures/node-diff');
const DIR = optionValue('--out') ? path.resolve(optionValue('--out')) : DEFAULT_DIR;

/** 全シナリオを Node 版に流して、書き出すファイルの内容 (ファイル名 -> テキスト) を作る。 */
export async function buildFixtures() {
  const files = new Map();
  const extra = Number(optionValue('--extra-fuzz') ?? 0);
  for (const scenario of [...scenarios, ...fuzzScenarios(extra)]) {
    const { profile, log } = await runScenario(scenario);
    files.set(`${scenario.name}.json`, { ...scenario, expect: { profile, log } });
  }
  files.set('foreground.json', {
    argv: foregroundArgv.map((argv) => ({ argv, name: commandNameFromArgv(argv) ?? null })),
    stats: foregroundStats.map((stat) => ({ stat, pgid: parseForegroundPgid(stat) ?? null })),
    sanitize: ['plain', '', '\u0007\u001b[31mred', 'a\u200Bb\u202Ec', 'x'.repeat(100), '日本語'.repeat(30)].map((name) => ({
      name,
      sanitized: sanitizeCommandName(name) ?? null,
    })),
  });
  return new Map([...files].map(([name, value]) => [name, `${JSON.stringify(value, null, 1)}\n`]));
}

const check = args.includes('--check');
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const files = await buildFixtures();
  if (!check) mkdirSync(DIR, { recursive: true });
  let stale = 0;
  for (const [name, text] of files) {
    const file = path.join(DIR, name);
    if (check) {
      let current = '';
      try {
        current = readFileSync(file, 'utf8');
      } catch {
        // 無ければ古いのと同じ
      }
      if (current !== text) {
        console.error(`${file} is out of date; run: node scripts/gen-rust-diff-fixtures.mjs`);
        stale++;
      }
    } else {
      writeFileSync(file, text);
    }
  }
  if (check && stale > 0) process.exit(1);
  console.log(check ? `${files.size} fixtures are up to date` : `wrote ${files.size} fixtures to ${DIR}`);
}
