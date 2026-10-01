import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  RpcMessageSchema,
  RpcResponseSchema,
  isNotification,
  isRequest,
  isResponse,
} from '../src/jsonrpc.js';
import { ErrorCode } from '../src/errors.js';

test('request / success / error / notification を判別できる', () => {
  const req = RpcMessageSchema.parse({ jsonrpc: '2.0', id: 1, method: 'server.info', params: {} });
  const ok = RpcMessageSchema.parse({ jsonrpc: '2.0', id: 'a', result: { x: 1 } });
  const err = RpcMessageSchema.parse({
    jsonrpc: '2.0',
    id: 2,
    error: { code: ErrorCode.PaneNotFound, message: 'nope' },
  });
  const note = RpcMessageSchema.parse({ jsonrpc: '2.0', method: 'event', params: { seq: 1 } });
  assert.deepEqual([isRequest(req), isResponse(req), isNotification(req)], [true, false, false]);
  assert.deepEqual([isRequest(ok), isResponse(ok), isNotification(ok)], [false, true, false]);
  assert.equal(isResponse(err), true);
  assert.deepEqual([isRequest(note), isResponse(note), isNotification(note)], [false, false, true]);
});

test('パースエラーの応答は id が null', () => {
  const r = RpcResponseSchema.safeParse({
    jsonrpc: '2.0',
    id: null,
    error: { code: ErrorCode.Parse, message: 'parse error' },
  });
  assert.equal(r.success, true);
});

test('異常系: jsonrpc 違い、id 型違い、result も error もない', () => {
  assert.equal(RpcMessageSchema.safeParse({ jsonrpc: '1.0', id: 1, method: 'x' }).success, false);
  assert.equal(RpcMessageSchema.safeParse({ jsonrpc: '2.0', id: {}, method: 'x' }).success, false);
  assert.equal(RpcMessageSchema.safeParse({ jsonrpc: '2.0', id: 1 }).success, false);
  assert.equal(RpcMessageSchema.safeParse({ jsonrpc: '2.0', id: null, result: 1 }).success, false);
});

test('result と error の両方を持つ応答は拒否', () => {
  const both = { jsonrpc: '2.0', id: 1, result: 1, error: { code: ErrorCode.Internal, message: 'x' } };
  assert.equal(RpcResponseSchema.safeParse(both).success, false);
  assert.equal(RpcMessageSchema.safeParse(both).success, false);
});
