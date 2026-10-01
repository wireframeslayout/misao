import assert from 'node:assert/strict';
import { test } from 'node:test';
import { PROTOCOL_VERSION, isCompatibleProtocolVersion } from '../src/version.js';

test('major が一致すれば互換', () => {
  assert.equal(isCompatibleProtocolVersion('0.1.0', '0.9.3'), true);
  assert.equal(isCompatibleProtocolVersion('1.2.0', '1.0.0'), true);
});

test('major が違えば非互換', () => {
  assert.equal(isCompatibleProtocolVersion('1.0.0', '2.0.0'), false);
  assert.equal(isCompatibleProtocolVersion(PROTOCOL_VERSION, '1.0.0'), false);
});

test('不正なバージョン文字列は例外', () => {
  assert.throws(() => isCompatibleProtocolVersion('x.y', '1.0.0'));
});
