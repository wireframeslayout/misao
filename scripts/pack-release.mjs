// 使い方: node scripts/pack-release.mjs --tag v0.1.0 [--out release]
// 事前に npm run build 済みで、全ワークスペースの version がタグと揃っている（scripts/set-version.mjs）こと。
// 公開用 package.json と dist の .js / .d.ts のみをステージングして npm pack する。
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { isPublishedFile, stripSourceMapComment } from './release/dist-file.mjs';
import { run } from './release/exec.mjs';
import {
  CHECKSUMS_FILE,
  RELEASE_PACKAGES,
  buildReleaseManifest,
  formatChecksums,
  parseReleaseTag,
  releaseFileNames,
  tarballName,
} from './release/manifest.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const { values } = parseArgs({ options: { tag: { type: 'string' }, out: { type: 'string', default: 'release' } } });
if (values.tag === undefined) throw new Error('usage: node scripts/pack-release.mjs --tag <vX.Y.Z> [--out <dir>]');
const outDir = path.resolve(values.out);
const { engines } = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8'));

function copyDist(from, to) {
  mkdirSync(to, { recursive: true });
  for (const entry of readdirSync(from, { withFileTypes: true })) {
    if (entry.isDirectory()) copyDist(path.join(from, entry.name), path.join(to, entry.name));
    else if (isPublishedFile(entry.name)) {
      const text = readFileSync(path.join(from, entry.name), 'utf8');
      writeFileSync(path.join(to, entry.name), stripSourceMapComment(text));
    }
  }
}

function packPackage({ name, dir }) {
  const source = JSON.parse(readFileSync(path.join(root, dir, 'package.json'), 'utf8'));
  const manifest = buildReleaseManifest({ source, tag: values.tag, engines });
  const distDir = path.join(root, dir, 'dist');
  for (const entry of [manifest.main, manifest.types]) {
    if (!existsSync(path.join(root, dir, entry))) {
      throw new Error(`${name}: ${dir}/${entry} not found (run npm run build)`);
    }
  }

  const staging = mkdtempSync(path.join(os.tmpdir(), 'misao-pack-'));
  try {
    copyDist(distDir, path.join(staging, 'dist'));
    copyFileSync(path.join(root, 'LICENSE'), path.join(staging, 'LICENSE'));
    copyFileSync(path.join(root, 'NOTICE'), path.join(staging, 'NOTICE'));
    writeFileSync(path.join(staging, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
    run('npm', ['pack', '--pack-destination', outDir], { cwd: staging });
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }

  const file = tarballName(name, manifest.version);
  if (!existsSync(path.join(outDir, file))) throw new Error(`expected ${file} in ${outDir}`);
  console.log(`packed ${path.join(values.out, file)}`);
}

mkdirSync(outDir, { recursive: true });
for (const pkg of RELEASE_PACKAGES) packPackage(pkg);

// CLI + デーモンのバンドル。node-pty は同梱せず、利用側の node_modules を使う。
run('node', [path.join(root, 'scripts/bundle-cli.mjs'), '--version', parseReleaseTag(values.tag).version, '--out', outDir]);

// tarball とバンドルの sha256 を SHA256SUMS にまとめる。
const checksums = releaseFileNames(parseReleaseTag(values.tag).version).map((fileName) => {
  const file = path.join(outDir, fileName);
  if (!existsSync(file)) throw new Error(`expected ${fileName} in ${outDir}`);
  return { fileName, sha256: createHash('sha256').update(readFileSync(file)).digest('hex') };
});
writeFileSync(path.join(outDir, CHECKSUMS_FILE), formatChecksums(checksums));
console.log(`wrote ${path.join(values.out, CHECKSUMS_FILE)}`);
