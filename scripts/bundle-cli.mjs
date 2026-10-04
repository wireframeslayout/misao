// 使い方: node scripts/bundle-cli.mjs [--version 0.1.0] [--out dist-release]
// あわせて、同梱した依存のライセンスをまとめた misao-<version>.LICENSES.txt を出力する。
// CLI とデーモンを 1 つの ESM ファイル misao-<version>.mjs にまとめる（node-pty はネイティブなので external）。
// 実行時は、このファイルの隣（または親）の node_modules/node-pty が解決される。
// ワークスペースのソースを @misao/source 条件で直接読むので、事前の npm run build は不要。
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { build } from 'esbuild';
import { formatLicenses, listBundledPackageDirs } from './release/licenses.mjs';
import { bundleName, licensesName, parseReleaseVersion } from './release/manifest.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const { values } = parseArgs({
  options: { version: { type: 'string' }, out: { type: 'string', default: 'dist-release' } },
});
const rootVersion = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
const { version } = parseReleaseVersion(values.version ?? rootVersion);
const outfile = path.resolve(values.out, bundleName(version));
const licensesFile = path.resolve(values.out, licensesName(version));

// エントリ（main.ts）の shebang は esbuild がそのまま先頭に残す。
const BANNER = [
  `/*! misao ${version} | Apache-2.0 | licenses of misao and the bundled packages: ${licensesName(version)} */`,
  // 依存の CJS が require() を使うため、ESM 内で require を用意する。
  "import { createRequire } from 'module'; const require = createRequire(import.meta.url);",
].join('\n');

const { metafile } = await build({
  entryPoints: [path.join(root, 'packages/cli/src/main.ts')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node24',
  conditions: ['@misao/source'],
  external: ['node-pty'],
  banner: { js: BANNER },
  define: { __MISAO_VERSION__: JSON.stringify(version) },
  metafile: true,
  outfile,
  logLevel: 'warning',
});
console.log(`bundled ${path.relative(process.cwd(), outfile)}`);

const LICENSE_FILE = /^(licen[sc]e|copying)(\.|$)/i;

// 公開物に LICENSE ファイルを持たないパッケージ（@xterm/headless など）は、scripts/release/third-party-licenses/ の
// 同梱ファイルを使う。同梱ファイルも無いときは、宣言されたライセンスとリポジトリだけを記して警告する（全文は捏造しない）。
const BUNDLED_LICENSES_DIR = path.join(root, 'scripts/release/third-party-licenses');

function bundledLicensePath(packageName) {
  return path.join(BUNDLED_LICENSES_DIR, `${packageName.replace(/^@/, '').replace('/', '-')}.LICENSE`);
}

function readLicenseText(manifest, dir) {
  const fileName = readdirSync(path.join(root, dir)).find((name) => LICENSE_FILE.test(name));
  if (fileName !== undefined) return readFileSync(path.join(root, dir, fileName), 'utf8');
  const bundled = bundledLicensePath(manifest.name);
  if (existsSync(bundled)) return readFileSync(bundled, 'utf8');
  const repository = typeof manifest.repository === 'string' ? manifest.repository : manifest.repository?.url;
  if (typeof repository !== 'string') throw new Error(`${manifest.name}: no LICENSE file and no repository in ${dir}`);
  console.warn(`warning: ${manifest.name} ships no LICENSE file; recorded its declared license only`);
  return `License: ${manifest.license} (declared in package.json).\nThe published package contains no license file; see ${repository}.`;
}

function readPackageLicense(dir) {
  const manifest = JSON.parse(readFileSync(path.join(root, dir, 'package.json'), 'utf8'));
  if (typeof manifest.license !== 'string') throw new Error(`${manifest.name}: package.json has no license`);
  return { name: manifest.name, version: manifest.version, license: manifest.license, text: readLicenseText(manifest, dir) };
}

// metafile のパスは cwd 基準。root 基準に直してからパッケージを列挙する。
const inputPaths = Object.keys(metafile.inputs).map((input) => path.relative(root, path.resolve(input)));
const packages = listBundledPackageDirs(inputPaths).map(readPackageLicense);
if (packages.length === 0) throw new Error('no bundled third-party packages found in the metafile');
if (!existsSync(path.join(root, 'NOTICE'))) throw new Error('NOTICE not found');
writeFileSync(
  licensesFile,
  formatLicenses({
    version,
    misaoLicense: readFileSync(path.join(root, 'LICENSE'), 'utf8'),
    misaoNotice: readFileSync(path.join(root, 'NOTICE'), 'utf8'),
    packages,
  }),
);
console.log(`licenses ${path.relative(process.cwd(), licensesFile)} (${packages.map((p) => p.name).join(', ')})`);
