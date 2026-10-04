import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { formatLicenses, listBundledPackageDirs } from '../release/licenses.mjs';

describe('listBundledPackageDirs', () => {
  it('lists node_modules packages once, sorted, and skips workspace sources', () => {
    assert.deepEqual(
      listBundledPackageDirs([
        'packages/cli/src/main.ts',
        'node_modules/zod/v4/index.js',
        'node_modules/zod/v4/core.js',
        'node_modules/@xterm/headless/lib-headless/a.js',
        'node_modules/a/node_modules/b/index.js',
      ]),
      ['node_modules/@xterm/headless', 'node_modules/a/node_modules/b', 'node_modules/zod'],
    );
  });
});

describe('formatLicenses', () => {
  it('puts misao first and then each package with its version and license', () => {
    const text = formatLicenses({
      version: '0.1.0',
      misaoLicense: 'APACHE TEXT\n',
      misaoNotice: 'NOTICE TEXT\n',
      packages: [{ name: 'zod', version: '4.0.0', license: 'MIT', text: 'MIT TEXT\n' }],
    });
    assert.ok(text.indexOf('misao (Apache-2.0)') < text.indexOf('zod@4.0.0 (MIT)'));
    for (const needle of ['APACHE TEXT', 'NOTICE TEXT', 'MIT TEXT', 'misao-0.1.0.mjs']) assert.ok(text.includes(needle));
  });
});
