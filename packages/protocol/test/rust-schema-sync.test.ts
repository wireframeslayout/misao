import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { buildProtocolJsonSchema } from '../src/schema.js';

const COMMITTED = new URL('../../../crates/misao-protocol/schema/protocol.json', import.meta.url);

test('crates/misao-protocol/schema/protocol.json は Zod スキーマの書き出しと一致する', () => {
  assert.equal(
    readFileSync(COMMITTED, 'utf8'),
    `${JSON.stringify(buildProtocolJsonSchema(), null, 2)}\n`,
    'out of date; run: node scripts/export-protocol-schema.mjs',
  );
});
