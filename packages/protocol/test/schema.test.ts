import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ErrorCode } from '../src/errors.js';
import { knownEventData } from '../src/events.js';
import { methods } from '../src/methods/index.js';
import { notifications } from '../src/notifications.js';
import { buildProtocolJsonSchema } from '../src/schema.js';
import { PROTOCOL_VERSION } from '../src/version.js';

const schema = buildProtocolJsonSchema();

test('全メソッド・通知・既知イベントが含まれる', () => {
  assert.deepEqual(Object.keys(schema.methods), Object.keys(methods));
  assert.deepEqual(Object.keys(schema.notifications), Object.keys(notifications));
  assert.deepEqual(Object.keys(schema.events), Object.keys(knownEventData));
  assert.deepEqual(schema.errors, { ...ErrorCode });
});

test('JSON.stringify できて protocolVersion が一致する', () => {
  assert.doesNotThrow(() => JSON.stringify(schema));
  assert.equal(schema.protocolVersion, PROTOCOL_VERSION);
});

test('pane.attach の mode は enum ["raw","cells"] で default が raw', () => {
  const mode = (schema.methods['pane.attach']?.params['properties'] as Record<string, Record<string, unknown>>)['mode'];
  assert.deepEqual(mode?.['enum'], ['raw', 'cells']);
  assert.equal(mode?.['default'], 'raw');
});

test('params (io: input) では default 付きのキーが required に入らない', () => {
  const required = schema.methods['pane.attach']?.params['required'];
  assert.deepEqual(required, ['paneId', 'clientId']);
});

test('result は未知のフィールドを許し、params は additionalProperties を縛らない', () => {
  assert.deepEqual(schema.methods['pane.open']?.result['additionalProperties'], {});
  assert.equal(schema.methods['pane.open']?.params['additionalProperties'], undefined);
});

test('pane.write / pane.set_label の params は anyOf で表現される', () => {
  assert.equal((schema.methods['pane.write']?.params['anyOf'] as unknown[]).length, 2);
  assert.equal((schema.methods['pane.set_label']?.params['anyOf'] as unknown[]).length, 2);
});

test('regex は pattern として出力される', () => {
  const props = schema.methods['pane.info']?.params['properties'] as Record<string, Record<string, unknown>>;
  assert.equal(props['paneId']?.['pattern'], '^p_[0-9A-HJKMNP-TV-Z]{26}$');
});

test('description が出力に含まれる', () => {
  assert.match(JSON.stringify(schema), /Time recorded by the daemon/);
  assert.match(JSON.stringify(schema.methods['pane.attach']), /1004 Unsupported/);
});

test('server.schema の result スキーマが生成結果を受け入れる', () => {
  assert.equal(methods['server.schema'].result.safeParse(schema).success, true);
});

test('pane.write の data と dataB64 の排他が schema 上で表現される', () => {
  const branches = schema.methods['pane.write']?.params['anyOf'] as { properties: Record<string, unknown> }[];
  assert.deepEqual(branches[0]?.properties['dataB64'], { not: {} });
  assert.deepEqual(branches[1]?.properties['data'], { not: {} });
});
