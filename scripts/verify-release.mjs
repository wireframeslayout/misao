// 使い方: node scripts/verify-release.mjs --tag v0.1.0 [--dir release]
// pack-release の成果物を別ディレクトリの一時プロジェクトに install し、
// `import { MisaoClient } from '@misao/sdk'` が実行時・型の両方で解決できることを確かめる。
// あわせて、CLI + デーモンのバンドルが SHA256SUMS と一致し、単体でスモークテストを通ることを確かめる。
// sdk の tarball は protocol をリリース URL で参照するため、overrides でローカルの protocol tarball に差し替える。
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { run } from './release/exec.mjs';
import {
  CHECKSUMS_FILE,
  bundleName,
  parseReleaseTag,
  releaseAssetUrl,
  licensesName,
  releaseFileNames,
  tarballName,
} from './release/manifest.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const { values } = parseArgs({ options: { tag: { type: 'string' }, dir: { type: 'string', default: 'release' } } });
if (values.tag === undefined) throw new Error('usage: node scripts/verify-release.mjs --tag <vX.Y.Z> [--dir <dir>]');
const { version } = parseReleaseTag(values.tag);
const releaseDir = path.resolve(values.dir);
const sdkTarball = path.join(releaseDir, tarballName('@misao/sdk', version));
const protocolTarball = path.join(releaseDir, tarballName('@misao/protocol', version));
for (const file of [sdkTarball, protocolTarball]) {
  if (!existsSync(file)) throw new Error(`${file} not found (run scripts/pack-release.mjs first)`);
}

const RUNTIME_CHECK = `import assert from 'node:assert/strict';
import { MisaoClient, resolveSocketPath } from '@misao/sdk';

assert.equal(typeof MisaoClient, 'function');
assert.equal(typeof resolveSocketPath, 'function');
console.log('runtime import ok');
`;

// MisaoClient 自体と、protocol 由来の型（request の method / params）が解決できることを tsc で確かめる。
const TYPE_CHECK = `import { MisaoClient } from '@misao/sdk';
import type { MisaoClientOptions } from '@misao/sdk';

const options: MisaoClientOptions = { socketPath: '/tmp/misao.sock' };
const client: MisaoClient = new MisaoClient(options);

export async function listPanes(): Promise<void> {
  await client.request('pane.list', {});
}
`;

const TSCONFIG = {
  compilerOptions: {
    target: 'ES2023',
    module: 'NodeNext',
    moduleResolution: 'NodeNext',
    strict: true,
    noEmit: true,
    types: ['node'],
  },
  include: ['check.ts'],
};

// tarball の中身を検査する: src / @misao/source / sourceMappingURL を含まず、sdk の protocol 依存が正規 URL であること。
function readPackedManifest(tarball) {
  const entries = run('tar', ['-tzf', tarball], { capture: true }).split('\n').filter(Boolean);
  const leaked = entries.filter((entry) => entry.startsWith('package/src/'));
  if (leaked.length > 0) throw new Error(`${path.basename(tarball)} contains source files: ${leaked.join(', ')}`);

  const contents = run('tar', ['-xzOf', tarball], { capture: true });
  if (contents.includes('//# sourceMappingURL=')) {
    throw new Error(`${path.basename(tarball)} still refers to source maps that are not shipped`);
  }

  const manifestText = run('tar', ['-xzOf', tarball, 'package/package.json'], { capture: true });
  if (manifestText.includes('@misao/source')) {
    throw new Error(`${path.basename(tarball)} package.json still has the @misao/source condition`);
  }
  return JSON.parse(manifestText);
}

function assertTarballContents() {
  readPackedManifest(protocolTarball);
  const expectedUrl = releaseAssetUrl(values.tag, tarballName('@misao/protocol', version));
  const actualUrl = readPackedManifest(sdkTarball).dependencies['@misao/protocol'];
  if (actualUrl !== expectedUrl) throw new Error(`@misao/protocol dependency is ${actualUrl}, expected ${expectedUrl}`);
}

function installedVersion(project, name) {
  return JSON.parse(readFileSync(path.join(project, 'node_modules', name, 'package.json'), 'utf8')).version;
}

function verifyInstall(project) {
  const { devDependencies } = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8'));
  writeFileSync(
    path.join(project, 'package.json'),
    JSON.stringify(
      {
        name: 'misao-release-verify',
        private: true,
        type: 'module',
        dependencies: { '@misao/sdk': `file:${sdkTarball}` },
        devDependencies: { '@types/node': devDependencies['@types/node'], typescript: devDependencies.typescript },
        overrides: { '@misao/protocol': `file:${protocolTarball}` },
      },
      null,
      2,
    ),
  );
  run('npm', ['install', '--no-audit', '--no-fund'], { cwd: project });

  for (const name of ['@misao/sdk', '@misao/protocol']) {
    const actual = installedVersion(project, name);
    if (actual !== version) throw new Error(`${name} installed as ${actual}, expected ${version}`);
  }

  writeFileSync(path.join(project, 'check.mjs'), RUNTIME_CHECK);
  writeFileSync(path.join(project, 'check.ts'), TYPE_CHECK);
  writeFileSync(path.join(project, 'tsconfig.json'), JSON.stringify(TSCONFIG, null, 2));
  run('node', ['check.mjs'], { cwd: project });
  run('npx', ['tsc', '-p', 'tsconfig.json'], { cwd: project });
}

// SHA256SUMS が公開ファイルをすべて、過不足なく、正しいハッシュで列挙していること。
function assertChecksums() {
  const sums = readFileSync(path.join(releaseDir, CHECKSUMS_FILE), 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const match = /^([0-9a-f]{64}) {2}(\S+)$/.exec(line);
      if (match === null) throw new Error(`malformed ${CHECKSUMS_FILE} line: ${line}`);
      return [match[2], match[1]];
    });
  const expected = releaseFileNames(version);
  const actualNames = sums.map(([name]) => name).sort();
  if (JSON.stringify(actualNames) !== JSON.stringify([...expected].sort())) {
    throw new Error(`${CHECKSUMS_FILE} lists [${actualNames}], expected [${[...expected].sort()}]`);
  }
  for (const [name, sha256] of sums) {
    const actual = createHash('sha256').update(readFileSync(path.join(releaseDir, name))).digest('hex');
    if (actual !== sha256) throw new Error(`${name} sha256 is ${actual}, ${CHECKSUMS_FILE} says ${sha256}`);
  }
}

function assertBundle() {
  const bundle = path.join(releaseDir, bundleName(version));
  if (!existsSync(bundle)) throw new Error(`${bundle} not found (run scripts/pack-release.mjs first)`);
  const licenses = path.join(releaseDir, licensesName(version));
  if (!existsSync(licenses)) throw new Error(`${licenses} not found (run scripts/pack-release.mjs first)`);
  const text = readFileSync(licenses, 'utf8');
  for (const needle of ['misao (Apache-2.0)', 'zod@', '@xterm/headless@']) {
    if (!text.includes(needle)) throw new Error(`${path.basename(licenses)} does not mention ${needle}`);
  }
  run('node', [path.join(root, 'scripts/smoke-bundle.mjs'), bundle]);
}

assertTarballContents();
assertChecksums();
assertBundle();
const base = mkdtempSync(path.join(os.tmpdir(), 'misao-verify-'));
try {
  const project = path.join(base, 'project');
  mkdirSync(project);
  verifyInstall(project);
  console.log(`verified ${path.basename(sdkTarball)}: runtime import and type resolution ok`);
} finally {
  rmSync(base, { recursive: true, force: true });
}
