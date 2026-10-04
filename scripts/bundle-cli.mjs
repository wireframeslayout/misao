// 使い方: node scripts/bundle-cli.mjs [--version 0.1.0] [--out dist-release]
// CLI とデーモンを 1 つの ESM ファイル misao-<version>.mjs にまとめる（node-pty はネイティブなので external）。
// 実行時は、このファイルの隣（または親）の node_modules/node-pty が解決される。
// ワークスペースのソースを @misao/source 条件で直接読むので、事前の npm run build は不要。
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { build } from 'esbuild';
import { bundleName, parseReleaseVersion } from './release/manifest.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const { values } = parseArgs({
  options: { version: { type: 'string' }, out: { type: 'string', default: 'dist-release' } },
});
const rootVersion = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
const { version } = parseReleaseVersion(values.version ?? rootVersion);
const outfile = path.resolve(values.out, bundleName(version));

// 依存の CJS が require() を使うため、ESM 内で require を用意する。
// エントリ（main.ts）の shebang は esbuild がそのまま先頭に残す。
const REQUIRE_BANNER = "import { createRequire } from 'module'; const require = createRequire(import.meta.url);";

await build({
  entryPoints: [path.join(root, 'packages/cli/src/main.ts')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node24',
  conditions: ['@misao/source'],
  external: ['node-pty'],
  banner: { js: REQUIRE_BANNER },
  outfile,
  logLevel: 'warning',
});
console.log(`bundled ${path.relative(process.cwd(), outfile)}`);
