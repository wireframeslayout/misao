import assert from 'node:assert/strict';
import { test } from 'node:test';
import { methods } from '../src/methods/index.js';
import type { MethodName } from '../src/methods/index.js';
import { PANE_ID, TS, ULID, WINDOW_ID } from './fixtures.js';

function params(name: MethodName, value: unknown): boolean {
  return methods[name].params.safeParse(value).success;
}

const ok = { ok: true };
const subscribe = { gap: false, head: 3, epoch: ULID };
const paneInfo = {
  paneId: PANE_ID, pid: 1, cmd: ['sh'], cwd: '/', workspace: 'w', window: { id: WINDOW_ID, name: 'x' },
  labels: {}, processState: 'running', exitCode: null, signal: null, agentState: 'idle',
  decidedBy: 'h', title: '', lastOutputAt: TS, cols: 80, rows: 24, clients: [], sizeOwner: null,
};
const win = { windowId: WINDOW_ID, name: 'w1', workspace: 'main' };
const ws = { name: 'main', windows: [win] };

test('全メソッドの代表的な params / result', () => {
  const cases: [MethodName, unknown, unknown][] = [
    ['server.info', {}, { protocolVersion: '0.1.0', pid: 1, epoch: ULID, uptimeSec: 1.5, paneCount: 0, eventHead: 0 }],
    ['server.schema', {}, { protocolVersion: '0.1.0', methods: {} }],
    ['workspace.list', {}, [ws]],
    ['workspace.create', { name: 'main' }, ws],
    ['workspace.close', { name: 'main' }, ok],
    ['workspace.rename', { name: 'a', newName: 'b' }, ok],
    ['window.create', { workspace: 'main', name: 'w1' }, win],
    ['window.close', { windowId: WINDOW_ID }, ok],
    ['window.rename', { windowId: WINDOW_ID, name: 'x' }, ok],
    ['window.focus', { windowId: WINDOW_ID, clientId: 'c1' }, ok],
    ['pane.open', { cmd: ['bash'], cwd: '/tmp', env: { A: 'b' }, ephemeralEnv: { TOKEN: 't' }, cols: 80, rows: 24, labels: { a: 'b' }, windowId: WINDOW_ID, preplace: [{ path: 'a/b.txt', content: 'x', mode: 0o644 }] }, { paneId: PANE_ID }],
    ['pane.info', { paneId: PANE_ID }, paneInfo],
    ['pane.list', { filter: { state: 'running', labels: { a: 'b' }, workspace: 'main' } }, [paneInfo]],
    ['pane.write', { paneId: PANE_ID, data: 'ls\n', source: 'hub', clientId: 'c' }, ok],
    ['pane.send_keys', { paneId: PANE_ID, keys: ['C-c', 'Enter'] }, ok],
    ['pane.resize', { paneId: PANE_ID, cols: 100, rows: 30 }, ok],
    ['pane.screen', { paneId: PANE_ID }, { text: '', cursor: { x: 0, y: 0 }, altScreen: false, title: '' }],
    ['pane.set_label', { paneId: PANE_ID, set: { a: 'b' }, unset: ['c'] }, { labels: { a: 'b' } }],
    ['pane.attach', { paneId: PANE_ID, clientId: 'c', replay: 'snapshot' }, { head: 5, oldest: 1, truncated: false }],
    ['pane.detach', { paneId: PANE_ID }, ok],
    ['pane.subscribe_lines', { paneId: PANE_ID, since: 0, epoch: ULID }, subscribe],
    ['pane.close', { paneId: PANE_ID }, ok],
    ['pane.respawn', { paneId: PANE_ID }, { paneId: PANE_ID }],
    ['events.subscribe', {}, subscribe],
  ];
  assert.equal(cases.length, Object.keys(methods).length);
  for (const [name, p, r] of cases) {
    assert.equal(params(name, p), true, `${name} params`);
    assert.equal(methods[name].result.safeParse(r).success, true, `${name} result`);
  }
});

test('result は未知のフィールドを受け入れ、params は捨てる', () => {
  assert.equal(methods['pane.open'].result.parse({ paneId: PANE_ID, extra: 1 })['extra'], 1);
  const p = methods['pane.close'].params.parse({ paneId: PANE_ID, extra: 1 });
  assert.deepEqual(p, { paneId: PANE_ID });
});

test('pane.open の境界値', () => {
  assert.equal(params('pane.open', { cmd: [] }), false);
  assert.equal(params('pane.open', { cmd: ['sh'], cols: 0 }), false);
  assert.equal(params('pane.open', { cmd: ['sh'], ephemeralEnv: { A: 1 } }), false);
  assert.equal(params('pane.open', { cmd: ['sh'], preplace: [{ path: '/etc/passwd', content: '' }] }), false);
  assert.equal(params('pane.open', { cmd: ['sh'], preplace: [{ path: '../x', content: '' }] }), false);
  assert.equal(params('pane.open', { cmd: ['sh'], preplace: [{ path: 'a/../x', content: '' }] }), false);
  assert.equal(params('pane.open', { cmd: ['sh'], preplace: [{ path: 'a/..b/x', content: '' }] }), true);
  for (const path of ['', '.', './x', 'a/', 'a//b', 'a\u0000b', 'a\\..\\x', 'C:\\x', 'C:/x', 'C:foo']) {
    assert.equal(params('pane.open', { cmd: ['sh'], preplace: [{ path, content: '' }] }), false, path);
  }
  assert.equal(params('pane.open', { cmd: ['sh'], preplace: [{ path: 'x', content: '', mode: 0o1000 }] }), false);
});

test('購読の since=-1 と不正な epoch は拒否', () => {
  assert.equal(params('events.subscribe', { since: 1, epoch: 'x' }), false);
  assert.equal(params('events.subscribe', { since: -1 }), false);
  assert.equal(params('pane.subscribe_lines', { paneId: PANE_ID, since: -1 }), false);
});

test('pane.write は data / dataB64 のどちらか一方', () => {
  assert.equal(params('pane.write', { paneId: PANE_ID, dataB64: 'AA==' }), true);
  assert.equal(params('pane.write', { paneId: PANE_ID }), false);
  assert.equal(params('pane.write', { paneId: PANE_ID, data: 'x', dataB64: 'AA==' }), false);
});

test('pane.attach の mode / replay', () => {
  const p = methods['pane.attach'].params.parse({ paneId: PANE_ID, clientId: 'c' });
  assert.equal(p.mode, 'raw');
  assert.equal(p.replay, 'none');
  assert.equal(params('pane.attach', { paneId: PANE_ID, clientId: 'c', mode: 'cells' }), true);
  assert.equal(params('pane.attach', { paneId: PANE_ID, clientId: 'c', replay: 'all' }), false);
});

test('send_keys の keys は 1 要素以上', () => {
  assert.equal(params('pane.send_keys', { paneId: PANE_ID, keys: [] }), false);
});

test('set_label は set か unset の少なくとも一方', () => {
  assert.equal(params('pane.set_label', { paneId: PANE_ID }), false);
  assert.equal(params('pane.set_label', { paneId: PANE_ID, set: { a: 'b' } }), true);
  assert.equal(params('pane.set_label', { paneId: PANE_ID, unset: ['a'] }), true);
});

test('pane.list の filter.state が不正なら拒否', () => {
  assert.equal(params('pane.list', { filter: { state: 'zombie' } }), false);
  assert.equal(params('pane.list', { filter: { state: 'unknown' } }), false, '送信側の enum は厳密');
  assert.equal(params('pane.list', { filter: { owner: 'x' } }), false, 'filter の未知キーは拒否');
  assert.equal(params('pane.list', {}), true);
});

test('pane.respawn は cmd を省略できる', () => {
  assert.equal(params('pane.respawn', { paneId: PANE_ID }), true);
  assert.equal(params('pane.respawn', { paneId: PANE_ID, cmd: ['sh'] }), true);
});
