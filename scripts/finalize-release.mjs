// 使い方: node scripts/finalize-release.mjs --tag v0.1.0 [--dir release]
// Node 版の資産 (tarball・バンドル・LICENSES) と Rust 版のアーカイブが release/ に揃ったことを確かめ、
// 全ファイルの sha256 を 1 つの SHA256SUMS にまとめる。過不足があれば例外にする (欠けたまま公開しない)。
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { CHECKSUMS_FILE, allReleaseFileNames, formatChecksums, parseReleaseTag } from './release/manifest.mjs';

const { values } = parseArgs({ options: { tag: { type: 'string' }, dir: { type: 'string', default: 'release' } } });
if (values.tag === undefined) throw new Error('usage: node scripts/finalize-release.mjs --tag <vX.Y.Z> [--dir <dir>]');
const { version } = parseReleaseTag(values.tag);
const dir = path.resolve(values.dir);

const expected = [...allReleaseFileNames(version)].sort();
const actual = readdirSync(dir).filter((name) => name !== CHECKSUMS_FILE).sort();
if (JSON.stringify(actual) !== JSON.stringify(expected)) {
  const missing = expected.filter((name) => !actual.includes(name));
  const unexpected = actual.filter((name) => !expected.includes(name));
  throw new Error(`release files do not match: missing [${missing}], unexpected [${unexpected}]`);
}

const entries = expected.map((fileName) => ({
  fileName,
  sha256: createHash('sha256').update(readFileSync(path.join(dir, fileName))).digest('hex'),
}));
writeFileSync(path.join(dir, CHECKSUMS_FILE), formatChecksums(entries));
console.log(`wrote ${CHECKSUMS_FILE} for ${entries.length} files`);
