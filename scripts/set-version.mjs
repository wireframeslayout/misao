// 使い方: node scripts/set-version.mjs <version>   例: 0.1.0 / 0.1.0-rc.1
// ルートと全ワークスペースの version、内部依存の参照、package-lock.json の該当エントリを揃える。
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { applyWorkspaceVersion, parseReleaseVersion } from './release/manifest.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function readJson(file) {
  return JSON.parse(readFileSync(file, 'utf8'));
}

function writeJson(file, value) {
  writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

const [versionArg] = process.argv.slice(2);
if (versionArg === undefined) throw new Error('usage: node scripts/set-version.mjs <version>');
const { version } = parseReleaseVersion(versionArg);

const workspaceDirs = readdirSync(path.join(root, 'packages')).map((dir) => `packages/${dir}`);
const workspaceNames = new Set(workspaceDirs.map((dir) => readJson(path.join(root, dir, 'package.json')).name));

for (const file of ['package.json', ...workspaceDirs.map((dir) => `${dir}/package.json`)]) {
  const manifest = readJson(path.join(root, file));
  writeJson(path.join(root, file), applyWorkspaceVersion({ manifest, version, workspaceNames }));
}

const lockFile = path.join(root, 'package-lock.json');
const lock = readJson(lockFile);
const packages = { ...lock.packages };
for (const key of ['', ...workspaceDirs]) {
  if (packages[key] === undefined) throw new Error(`package-lock.json has no entry for "${key}"`);
  packages[key] = applyWorkspaceVersion({ manifest: packages[key], version, workspaceNames });
}
writeJson(lockFile, { ...lock, version, packages });

console.log(`set version ${version} on root + ${workspaceDirs.length} workspaces`);
