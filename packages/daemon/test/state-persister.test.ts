import { test } from 'node:test';
import assert from 'node:assert/strict';
import { StatePersister } from '../src/state-persister.js';
import type { PersistedState } from '../src/persistence.js';
import { newPaneId, newWindowId } from '../src/ulid.js';

function setup(state: PersistedState): { persister: StatePersister; saved: PersistedState[]; logs: string[]; fail: { on: boolean } } {
  const saved: PersistedState[] = [];
  const logs: string[] = [];
  const fail = { on: false };
  const persister = new StatePersister({
    snapshot: () => state,
    save: (s) => {
      if (fail.on) throw new Error('disk full');
      saved.push(s);
    },
    log: (m) => logs.push(m),
  });
  return { persister, saved, logs, fail };
}

const empty: PersistedState = { version: 1, workspaces: [], panes: [] };

test('saveNow は存在しない window を指す pane を含む状態を書かずに例外にする', () => {
  const orphan: PersistedState = {
    version: 1,
    workspaces: [],
    panes: [{ paneId: newPaneId(), windowId: newWindowId(), cmd: ['sh'], cwd: '/', env: {}, labels: {}, cols: 80, rows: 24 }],
  };
  const { persister, saved } = setup(orphan);
  assert.throws(() => persister.saveNow(), /inconsistent/);
  assert.equal(saved.length, 0);
});

test('saveNow は保存の失敗をそのまま投げ、saveSoon は保存を遅らせて flush で 1 回にまとめる', () => {
  const { persister, saved, fail } = setup(empty);
  fail.on = true;
  assert.throws(() => persister.saveNow(), /disk full/);
  fail.on = false;
  persister.saveSoon();
  persister.saveSoon();
  assert.equal(saved.length, 0);
  persister.flush();
  assert.equal(saved.length, 1);
  persister.flush(); // 遅らせている保存が無ければ何もしない
  assert.equal(saved.length, 1);
});

test('遅らせた保存の失敗はログだけで、例外にしない', () => {
  const { persister, logs, fail } = setup(empty);
  fail.on = true;
  persister.saveSoon();
  assert.doesNotThrow(() => persister.flush());
  assert.match(logs.join('\n'), /failed to persist state: disk full/);
});

test('saveNow は遅らせていた保存を取り消す (同じ内容を含むため)', () => {
  const { persister, saved } = setup(empty);
  persister.saveSoon();
  persister.saveNow();
  persister.flush();
  assert.equal(saved.length, 1);
});

test('保存に失敗した saveNow は、遅らせていた保存の予約を残す (flush で保存される)', () => {
  const { persister, saved, fail } = setup(empty);
  persister.saveSoon();
  fail.on = true;
  assert.throws(() => persister.saveNow(), /disk full/);
  fail.on = false;
  persister.flush();
  assert.equal(saved.length, 1);
});

test('タイマーでの保存が失敗しても、次の saveSoon で予約し直され、再びタイマーで保存される', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { persister, saved, logs, fail } = setup(empty);
  fail.on = true;
  persister.saveSoon();
  t.mock.timers.tick(1000);
  assert.equal(saved.length, 0);
  assert.match(logs.join('\n'), /disk full/);
  fail.on = false;
  persister.saveSoon();
  t.mock.timers.tick(1000);
  assert.equal(saved.length, 1);
  persister.flush(); // 保存済みなので何もしない
  assert.equal(saved.length, 1);
});

test('タイマーでの保存が失敗したまま shutdown しても、flush で保存される', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { persister, saved, fail } = setup(empty);
  fail.on = true;
  persister.saveSoon();
  t.mock.timers.tick(1000);
  fail.on = false;
  persister.flush();
  assert.equal(saved.length, 1);
});
