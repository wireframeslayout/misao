import assert from 'node:assert/strict';
import { test } from 'node:test';
import { PaneInfoSchema } from '../src/pane-info.js';
import { PANE_ID, TS, WINDOW_ID } from './fixtures.js';

const base = {
  paneId: PANE_ID,
  pid: 123,
  cmd: ['bash'],
  cwd: '/tmp',
  workspace: 'main',
  window: { id: WINDOW_ID, name: 'w1' },
  labels: { owner: 'me' },
  processState: 'running',
  exitCode: null,
  signal: null,
  agentState: 'idle',
  decidedBy: 'heuristic',
  title: '',
  lastOutputAt: TS,
  cols: 80,
  rows: 24,
  clients: ['c1'],
  sizeOwner: 'c1',
};

test('fgCommand は省略できる', () => {
  assert.equal(PaneInfoSchema.safeParse(base).success, true);
  assert.equal(PaneInfoSchema.safeParse({ ...base, fgCommand: 'vim' }).success, true);
});

test('stopped のとき pid は null', () => {
  const r = PaneInfoSchema.parse({ ...base, processState: 'stopped', pid: null, lastOutputAt: null });
  assert.equal(r.pid, null);
});

test('未知のフィールドを受け入れる', () => {
  const r = PaneInfoSchema.parse({ ...base, future: 1 });
  assert.equal(r['future'], 1);
});

test('未知の processState / agentState は "unknown" として読む', () => {
  assert.equal(PaneInfoSchema.parse({ ...base, processState: 'dead' }).processState, 'unknown');
  assert.equal(PaneInfoSchema.parse({ ...base, agentState: 'busy' }).agentState, 'unknown');
});

test('必須フィールド欠落は拒否', () => {
  const { cwd: _cwd, ...rest } = base;
  assert.equal(PaneInfoSchema.safeParse(rest).success, false);
});
