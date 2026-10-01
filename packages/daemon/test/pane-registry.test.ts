import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ErrorCode } from '@misao/protocol';
import type { Pane } from '../src/pane.js';
import { PaneRegistry } from '../src/pane-registry.js';
import type { PaneRecord } from '../src/pane-registry.js';
import { RpcFailure } from '../src/rpc-error.js';
import { newPaneId, newWindowId } from '../src/ulid.js';

const WIN_A = newWindowId();
const WIN_B = newWindowId();

function record(windowId: string, labels: Record<string, string> = {}): PaneRecord {
  return { paneId: newPaneId(), windowId, cmd: ['sh'], cwd: '/tmp', env: {}, labels, cols: 100, rows: 30 };
}

/** registry が参照する範囲だけを持つ生きている Pane の代役。 */
function fakeLive(cols: number, rows: number): Pane {
  return { state: 'running', cols, rows } as unknown as Pane;
}

test('filter: state / labels / window を AND で絞る', () => {
  const reg = new PaneRegistry();
  const a = record(WIN_A, { origin: 'terminal', owner: 'x' });
  const b = record(WIN_B, { origin: 'terminal' });
  const c = record(WIN_A);
  reg.add(a, fakeLive(80, 24));
  reg.restoreStopped([b, c]);
  const ids = (entries: ReturnType<PaneRegistry['filter']>): string[] => entries.map((e) => e.record.paneId);
  assert.deepEqual(ids(reg.filter(undefined, undefined, undefined)), [a.paneId, b.paneId, c.paneId]);
  assert.deepEqual(ids(reg.filter('running', undefined, undefined)), [a.paneId]);
  assert.deepEqual(ids(reg.filter('stopped', { origin: 'terminal' }, undefined)), [b.paneId]);
  assert.deepEqual(ids(reg.filter(undefined, { origin: 'terminal' }, new Set([WIN_A]))), [a.paneId]);
  assert.deepEqual(ids(reg.filter(undefined, undefined, new Set())), []);
});

test('setLabels は新しい labels を返し、元の record と以前の labels を書き換えない', () => {
  const reg = new PaneRegistry();
  const r = record(WIN_A, { a: '1', b: '2' });
  reg.restoreStopped([r]);
  const before = reg.get(r.paneId)!.record;
  const labels = reg.setLabels(r.paneId, { b: '3', c: '4' }, ['a']);
  assert.deepEqual(labels, { b: '3', c: '4' });
  assert.deepEqual(before.labels, { a: '1', b: '2' });
  assert.notEqual(reg.get(r.paneId)!.record, before);
  assert.deepEqual(reg.get(r.paneId)!.record.labels, labels);
  assert.deepEqual(reg.setLabels(r.paneId, undefined, ['c']), { b: '3' });
  assert.throws(() => reg.setLabels(newPaneId(), {}, []), (e: unknown) => e instanceof RpcFailure && e.code === ErrorCode.PaneNotFound);
});

test('stopped の info は record から作る (pid / exitCode / signal / lastOutputAt は null)', () => {
  const reg = new PaneRegistry();
  const r = record(WIN_A, { owner: 'x' });
  reg.restoreStopped([r]);
  const info = reg.info(r.paneId, 'default', { id: WIN_A, name: 'default' });
  assert.equal(info.processState, 'stopped');
  assert.equal(info.pid, null);
  assert.equal(info.exitCode, null);
  assert.equal(info.signal, null);
  assert.equal(info.agentState, 'unknown');
  assert.equal(info.decidedBy, 'none');
  assert.equal(info.lastOutputAt, null);
  assert.deepEqual(info.clients, []);
  assert.equal(info.sizeOwner, null);
  assert.deepEqual([info.cols, info.rows, info.cwd, info.cmd], [100, 30, '/tmp', ['sh']]);
  assert.deepEqual(info.labels, { owner: 'x' });
});

test('toPersisted は live の現在のサイズを反映し、remove した pane は含めない', () => {
  const reg = new PaneRegistry();
  const live = record(WIN_A);
  const stopped = record(WIN_A);
  reg.add(live, fakeLive(120, 40));
  reg.restoreStopped([stopped]);
  const saved = reg.toPersisted();
  assert.deepEqual(saved.map((r) => [r.cols, r.rows]), [[120, 40], [100, 30]]);
  assert.equal(live.cols, 100);
  reg.remove(stopped.paneId);
  assert.deepEqual(reg.toPersisted().map((r) => r.paneId), [live.paneId]);
  assert.equal(reg.size, 1);
});
