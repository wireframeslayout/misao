import assert from 'node:assert/strict';
import { test } from 'node:test';
import { notifications } from '@misao/protocol';
import { readHotNotification } from '../src/hot-notification.js';

const PANE_ID = 'p_01ARZ3NDEKTSV4RRFFQ69G5FAV';
const TS = '2026-01-01T00:00:00.000Z';

const line = (params: Record<string, unknown>): unknown => ({ jsonrpc: '2.0', method: 'pane.line', params });
const output = (params: Record<string, unknown>): unknown => ({ jsonrpc: '2.0', method: 'pane.output', params });
const validLine = { seq: 1, ts: TS, paneId: PANE_ID, text: 'hi' };
const validOutput = { seq: 1, ts: TS, paneId: PANE_ID, dataB64: 'QQ==' };

interface Case {
  name: string;
  json: () => unknown;
  /** Zod の完全な検証との関係。same: 合否も出力も一致 / relaxed: 最小確認だけ通す (書式は見ない)。 */
  expect: 'valid' | 'invalid' | 'relaxed';
}

const cases: Case[] = [
  { name: 'pane.line', json: () => line({ ...validLine }), expect: 'valid' },
  { name: 'pane.line: 余分なフィールドを保持する', json: () => line({ ...validLine, extra: { a: 1 } }), expect: 'valid' },
  { name: 'pane.line: seq 0', json: () => line({ ...validLine, seq: 0 }), expect: 'valid' },
  { name: 'pane.line: seq 欠落', json: () => line({ ts: TS, paneId: PANE_ID, text: 'hi' }), expect: 'invalid' },
  { name: 'pane.line: seq が小数', json: () => line({ ...validLine, seq: 1.5 }), expect: 'invalid' },
  { name: 'pane.line: seq が負', json: () => line({ ...validLine, seq: -1 }), expect: 'invalid' },
  { name: 'pane.line: seq が文字列', json: () => line({ ...validLine, seq: '1' }), expect: 'invalid' },
  { name: 'pane.line: text が数値', json: () => line({ ...validLine, text: 1 }), expect: 'invalid' },
  { name: 'pane.line: text 欠落', json: () => line({ seq: 1, ts: TS, paneId: PANE_ID }), expect: 'invalid' },
  { name: 'pane.line: paneId 欠落', json: () => line({ seq: 1, ts: TS, text: 'hi' }), expect: 'invalid' },
  { name: 'pane.line: ts が数値', json: () => line({ ...validLine, ts: 0 }), expect: 'invalid' },
  { name: 'pane.line: ts の書式違い (最小確認は通す)', json: () => line({ ...validLine, ts: 'yesterday' }), expect: 'relaxed' },
  { name: 'pane.line: paneId の書式違い (最小確認は通す)', json: () => line({ ...validLine, paneId: 'x' }), expect: 'relaxed' },
  { name: 'pane.output', json: () => output({ ...validOutput }), expect: 'valid' },
  { name: 'pane.output: replay snapshot', json: () => output({ ...validOutput, replay: 'snapshot' }), expect: 'valid' },
  { name: 'pane.output: replay unknown', json: () => output({ ...validOutput, replay: 'unknown' }), expect: 'valid' },
  { name: 'pane.output: 知らない replay は unknown に読む', json: () => output({ ...validOutput, replay: 'future' }), expect: 'valid' },
  { name: 'pane.output: replay が数値でも unknown に読む', json: () => output({ ...validOutput, replay: 3 }), expect: 'valid' },
  { name: 'pane.output: replay null も unknown に読む', json: () => output({ ...validOutput, replay: null }), expect: 'valid' },
  { name: 'pane.output: 余分なフィールドを保持する', json: () => output({ ...validOutput, extra: 'x' }), expect: 'valid' },
  { name: 'pane.output: dataB64 欠落', json: () => output({ seq: 1, ts: TS, paneId: PANE_ID }), expect: 'invalid' },
  { name: 'pane.output: dataB64 が数値', json: () => output({ ...validOutput, dataB64: 1 }), expect: 'invalid' },
  { name: 'pane.output: seq が 0 未満', json: () => output({ ...validOutput, seq: -3 }), expect: 'invalid' },
];

for (const c of cases) {
  test(`最小確認と Zod の突き合わせ: ${c.name}`, () => {
    const json = c.json() as { method: 'pane.line' | 'pane.output'; params: unknown };
    const zod = notifications[json.method].safeParse(structuredClone(json.params));
    const hot = readHotNotification(c.json());
    assert.ok(hot, 'ホットパスの通知として扱う');
    if (c.expect === 'invalid') {
      assert.equal(zod.success, false, 'Zod も不正とする');
      assert.equal(hot.ok, false);
      return;
    }
    assert.equal(hot.ok, true);
    if (c.expect === 'valid') {
      assert.equal(zod.success, true);
      assert.deepEqual(hot.ok && hot.notification.params, zod.data, '出力が Zod と一致する');
    } else {
      assert.equal(zod.success, false, 'Zod だけが書式違反として弾く');
    }
  });
}

test('ホットパスの通知でないものは undefined (完全な検証に回す)', () => {
  const rejected: unknown[] = [
    null,
    'text',
    [],
    { method: 'pane.line', params: validLine }, // jsonrpc 欠落
    { jsonrpc: '1.0', method: 'pane.line', params: validLine },
    { jsonrpc: '2.0', id: 1, method: 'pane.line', params: validLine }, // id があれば通知でない
    { jsonrpc: '2.0', method: 'pane.line' }, // params 欠落
    { jsonrpc: '2.0', method: 'pane.line', params: [] },
    { jsonrpc: '2.0', method: 'pane.line', params: null },
    { jsonrpc: '2.0', method: 'event', params: { seq: 1 } },
    { jsonrpc: '2.0', id: 1, result: {} },
  ];
  for (const json of rejected) assert.equal(readHotNotification(json), undefined, JSON.stringify(json));
});
