// 使い方: node scripts/pack-binary.mjs --tag v0.1.0 --target x86_64-unknown-linux-musl --bin target/<triple>/release/misao --licenses THIRD-PARTY-LICENSES.txt [--out release]
// Rust 版の単一バイナリを、ライセンス表記とともに misao-<version>-<target>.tar.gz (Windows は .zip) にまとめる。
// アーカイブは misao-<version>-<target>/ を 1 つ含み、その直下に misao (Windows は misao.exe) / LICENSE / NOTICE / THIRD-PARTY-LICENSES.txt を置く。
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { createArchive } from './release/archive.mjs';
import {
  BINARY_LICENSES_FILE,
  BINARY_TARGETS,
  binaryArchiveDir,
  binaryArchiveName,
  binaryExecutableName,
  isWindowsTarget,
  parseReleaseTag,
} from './release/manifest.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const USAGE = 'usage: node scripts/pack-binary.mjs --tag <vX.Y.Z> --target <triple> --bin <file> --licenses <file> [--out <dir>]';

const { values } = parseArgs({
  options: {
    tag: { type: 'string' },
    target: { type: 'string' },
    bin: { type: 'string' },
    licenses: { type: 'string' },
    out: { type: 'string', default: 'release' },
  },
});
for (const name of ['tag', 'target', 'bin', 'licenses']) {
  if (values[name] === undefined) throw new Error(USAGE);
}
const { version } = parseReleaseTag(values.tag);
if (!BINARY_TARGETS.some(({ target }) => target === values.target)) {
  throw new Error(`unknown target ${values.target} (expected one of ${BINARY_TARGETS.map(({ target }) => target).join(', ')})`);
}
const windows = isWindowsTarget(values.target);
for (const file of [values.bin, values.licenses]) {
  if (!existsSync(file)) throw new Error(`${file} not found`);
}

const outDir = path.resolve(values.out);
mkdirSync(outDir, { recursive: true });
const staging = mkdtempSync(path.join(os.tmpdir(), 'misao-pack-binary-'));
try {
  const dirName = binaryArchiveDir(version, values.target);
  const dir = path.join(staging, dirName);
  mkdirSync(dir);
  const executable = path.join(dir, binaryExecutableName(values.target));
  copyFileSync(values.bin, executable);
  if (!windows) chmodSync(executable, 0o755);
  copyFileSync(path.join(root, 'LICENSE'), path.join(dir, 'LICENSE'));
  copyFileSync(path.join(root, 'NOTICE'), path.join(dir, 'NOTICE'));
  copyFileSync(values.licenses, path.join(dir, BINARY_LICENSES_FILE));

  const archiveName = binaryArchiveName(version, values.target);
  createArchive({ archive: path.join(outDir, archiveName), cwd: staging, dir: dirName, windows });
  console.log(`packed ${path.join(values.out, archiveName)}`);
} finally {
  rmSync(staging, { recursive: true, force: true });
}
