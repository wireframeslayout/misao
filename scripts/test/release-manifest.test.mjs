import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  applyWorkspaceVersion,
  buildReleaseManifest,
  parseReleaseTag,
  releaseAssetUrl,
  tarballName,
} from '../release/manifest.mjs';

const sdkSource = {
  name: '@misao/sdk',
  version: '0.1.0',
  private: true,
  type: 'module',
  license: 'Apache-2.0',
  main: 'dist/index.js',
  types: 'dist/index.d.ts',
  exports: { '.': { '@misao/source': './src/index.ts', types: './dist/index.d.ts', default: './dist/index.js' } },
  scripts: { test: 'node --test' },
  dependencies: { '@misao/protocol': '0.1.0', zod: '^4.6.5' },
};

describe('parseReleaseTag', () => {
  it('parses a stable tag', () => {
    assert.deepEqual(parseReleaseTag('v1.2.3'), { tag: 'v1.2.3', version: '1.2.3', isPrerelease: false });
  });

  it('treats a hyphenated tag as pre-release', () => {
    assert.deepEqual(parseReleaseTag('v1.2.3-rc.1'), { tag: 'v1.2.3-rc.1', version: '1.2.3-rc.1', isPrerelease: true });
  });

  for (const bad of ['1.2.3', 'v1.2', 'v1.2.3-', 'v1.2.3+build', 'vfoo', 'v1.2.3-rc..1']) {
    it(`rejects ${bad}`, () => {
      assert.throws(() => parseReleaseTag(bad), /invalid/);
    });
  }
});

describe('tarballName / releaseAssetUrl', () => {
  it('drops the scope the way npm pack does', () => {
    assert.equal(tarballName('@misao/protocol', '0.1.0'), 'misao-protocol-0.1.0.tgz');
    assert.equal(tarballName('@misao/sdk', '0.1.0-rc.1'), 'misao-sdk-0.1.0-rc.1.tgz');
  });

  it('builds the GitHub Releases download URL', () => {
    assert.equal(
      releaseAssetUrl('v0.1.0', 'misao-protocol-0.1.0.tgz'),
      'https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-protocol-0.1.0.tgz',
    );
  });
});

describe('buildReleaseManifest', () => {
  it('points @misao/protocol at the same release tarball and keeps zod as semver', () => {
    const manifest = buildReleaseManifest({ source: sdkSource, tag: 'v0.1.0', engines: { node: '>=24' } });
    assert.deepEqual(manifest.dependencies, {
      '@misao/protocol':
        'https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-protocol-0.1.0.tgz',
      zod: '^4.6.5',
    });
  });

  it('drops the @misao/source export, scripts and private', () => {
    const manifest = buildReleaseManifest({ source: sdkSource, tag: 'v0.1.0', engines: { node: '>=24' } });
    assert.deepEqual(manifest.exports, { '.': { types: './dist/index.d.ts', default: './dist/index.js' } });
    assert.equal(manifest.scripts, undefined);
    assert.equal(manifest.private, undefined);
    assert.deepEqual(manifest.engines, { node: '>=24' });
  });

  it('does not mutate the source manifest', () => {
    const before = structuredClone(sdkSource);
    buildReleaseManifest({ source: sdkSource, tag: 'v0.1.0', engines: { node: '>=24' } });
    assert.deepEqual(sdkSource, before);
  });

  it('fails when the package version does not match the tag', () => {
    assert.throws(
      () => buildReleaseManifest({ source: sdkSource, tag: 'v0.2.0', engines: { node: '>=24' } }),
      /requires 0\.2\.0/,
    );
  });

  it('fails on an unreleased internal dependency', () => {
    const source = { ...sdkSource, dependencies: { '@misao/daemon': '0.1.0' } };
    assert.throws(() => buildReleaseManifest({ source, tag: 'v0.1.0', engines: { node: '>=24' } }), /@misao\/daemon/);
  });
});

describe('applyWorkspaceVersion', () => {
  const workspaceNames = new Set(['@misao/protocol', '@misao/sdk', 'misao']);

  it('updates the version and internal references only', () => {
    const next = applyWorkspaceVersion({
      manifest: { ...sdkSource, version: '0.0.0', dependencies: { '@misao/protocol': '0.0.0', zod: '^4.6.5' } },
      version: '0.3.0',
      workspaceNames,
    });
    assert.equal(next.version, '0.3.0');
    assert.deepEqual(next.dependencies, { '@misao/protocol': '0.3.0', zod: '^4.6.5' });
  });

  it('leaves manifests without dependencies alone and does not mutate the input', () => {
    const manifest = { name: 'misao-monorepo', version: '0.0.0' };
    const next = applyWorkspaceVersion({ manifest, version: '0.3.0', workspaceNames });
    assert.deepEqual(next, { name: 'misao-monorepo', version: '0.3.0' });
    assert.equal(manifest.version, '0.0.0');
  });
});
