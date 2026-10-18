import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { describe, it } from 'node:test';
import {
  BINARY_TARGETS,
  allReleaseFileNames,
  applyCargoVersion,
  binaryArchiveMembers,
  binaryArchiveName,
  binaryFileNames,
  releaseFileNames,
} from '../release/manifest.mjs';

const CARGO_TOML = `[workspace]
members = ["crates/*"]

[workspace.package]
version = "0.0.0"
edition = "2024"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
`;

const lockEntry = (name, version) => `[[package]]\nname = "${name}"\nversion = "${version}"\ndependencies = []\n`;
const CARGO_LOCK = [
  lockEntry('misao', '0.0.0'),
  lockEntry('misao-daemon', '0.0.0'),
  lockEntry('misao-protocol', '0.0.0'),
  lockEntry('serde', '1.0.0'),
  lockEntry('misao-other', '0.0.0'),
].join('\n');

describe('binaryArchiveName', () => {
  it('uses tar.gz for Unix targets and zip for Windows', () => {
    assert.equal(binaryArchiveName('0.2.0', 'x86_64-unknown-linux-musl'), 'misao-0.2.0-x86_64-unknown-linux-musl.tar.gz');
    assert.equal(binaryArchiveName('0.2.0', 'aarch64-apple-darwin'), 'misao-0.2.0-aarch64-apple-darwin.tar.gz');
    assert.equal(binaryArchiveName('0.2.0-rc.1', 'x86_64-pc-windows-msvc'), 'misao-0.2.0-rc.1-x86_64-pc-windows-msvc.zip');
  });

  it('names a .exe only for Windows', () => {
    assert.deepEqual(binaryArchiveMembers('x86_64-pc-windows-msvc'), ['misao.exe', 'LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.txt']);
    assert.deepEqual(binaryArchiveMembers('aarch64-apple-darwin'), ['misao', 'LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.txt']);
  });
});

describe('allReleaseFileNames', () => {
  it('is the Node assets plus one archive per target, without duplicates', () => {
    const all = allReleaseFileNames('0.2.0');
    assert.equal(all.length, releaseFileNames('0.2.0').length + BINARY_TARGETS.length);
    assert.equal(new Set(all).size, all.length);
    for (const name of binaryFileNames('0.2.0')) assert.ok(all.includes(name));
    assert.ok(all.includes('misao-0.2.0.mjs'));
    assert.ok(all.includes('misao-0.2.0.LICENSES.txt'));
  });
});

describe('applyCargoVersion', () => {
  it('sets the workspace version and only the workspace crates in Cargo.lock', () => {
    const next = applyCargoVersion({ cargoToml: CARGO_TOML, cargoLock: CARGO_LOCK, version: '1.2.3-rc.1' });
    assert.match(next.cargoToml, /\[workspace\.package\]\nversion = "1\.2\.3-rc\.1"\nedition/);
    assert.match(next.cargoToml, /serde = \{ version = "1"/);
    for (const name of ['misao', 'misao-daemon', 'misao-protocol']) {
      assert.ok(next.cargoLock.includes(`name = "${name}"\nversion = "1.2.3-rc.1"`), name);
    }
    assert.ok(next.cargoLock.includes('name = "serde"\nversion = "1.0.0"'));
    assert.ok(next.cargoLock.includes('name = "misao-other"\nversion = "0.0.0"'));
  });

  it('keeps CRLF line endings (a Windows checkout)', () => {
    const crlf = (text) => text.replaceAll('\n', '\r\n');
    const next = applyCargoVersion({ cargoToml: crlf(CARGO_TOML), cargoLock: crlf(CARGO_LOCK), version: '1.2.3' });
    assert.ok(next.cargoToml.includes('[workspace.package]\r\nversion = "1.2.3"\r\nedition'));
    assert.ok(next.cargoLock.includes('name = "misao-daemon"\r\nversion = "1.2.3"\r\n'));
  });

  it('fails instead of silently skipping when a workspace crate is missing from Cargo.lock', () => {
    const lock = CARGO_LOCK.replace(lockEntry('misao-daemon', '0.0.0'), '');
    assert.throws(() => applyCargoVersion({ cargoToml: CARGO_TOML, cargoLock: lock, version: '1.0.0' }), /expected 3/);
  });

  it('fails when Cargo.toml has no workspace version', () => {
    assert.throws(() => applyCargoVersion({ cargoToml: '[workspace]\n', cargoLock: CARGO_LOCK, version: '1.0.0' }), /workspace\.package/);
    assert.throws(
      () => applyCargoVersion({ cargoToml: '[workspace.package]\nedition = "2024"\n', cargoLock: CARGO_LOCK, version: '1.0.0' }),
      /no version line/,
    );
  });
});

describe('release.yml', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8');
  const aboutToml = readFileSync(new URL('../../about.toml', import.meta.url), 'utf8');

  it('builds exactly the BINARY_TARGETS, on their runners', () => {
    const matrix = [...workflow.matchAll(/- \{ target: (\S+), runner: ([^,\s}]+)/g)].map(([, target, runner]) => ({ target, runner }));
    assert.deepEqual(matrix, BINARY_TARGETS);
  });

  it('lists the same targets in about.toml', () => {
    const listed = [...aboutToml.matchAll(/^ {2}"([^"]+)",$/gm)].map(([, value]) => value).filter((value) => value.includes('-'));
    for (const { target } of BINARY_TARGETS) assert.ok(listed.includes(target), target);
  });

  it('pins every action to a full commit SHA and the Rust toolchain to a version', () => {
    const uses = [...workflow.matchAll(/^\s+- uses: (\S+)/gm)].map(([, ref]) => ref);
    assert.ok(uses.length > 0);
    for (const ref of uses) assert.match(ref, /@[0-9a-f]{40}$/, ref);
    for (const [, name] of workflow.matchAll(/toolchain: (\S+)/g)) assert.match(name, /^\d+\.\d+\.\d+$/);
    assert.doesNotMatch(workflow, /rust-toolchain@(?!683b)/);
  });

  it('publishes only for tag pushes', () => {
    assert.match(workflow, /publish:\n {4}name: [^\n]+\n {4}if: github\.event_name == 'push'/);
  });
});
