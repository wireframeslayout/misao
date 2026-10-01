import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ErrorCode } from '@misao/protocol';
import { Layout } from '../src/layout.js';
import { RpcFailure } from '../src/rpc-error.js';
import { ulid } from '../src/ulid.js';

function expectFailure(fn: () => unknown, code: number): void {
  assert.throws(fn, (e: unknown) => e instanceof RpcFailure && e.code === code);
}

test('workspace は名前で一意、window 名は重複できる', () => {
  const layout = new Layout();
  layout.createWorkspace('a');
  expectFailure(() => layout.createWorkspace('a'), ErrorCode.AlreadyExists);
  const w1 = layout.createWindow('a', 'w');
  const w2 = layout.createWindow('a', 'w');
  assert.notEqual(w1.windowId, w2.windowId);
  assert.match(w1.windowId, /^w_[0-9A-Z]{26}$/);
  expectFailure(() => layout.createWindow('none', 'w'), ErrorCode.WorkspaceNotFound);
});

test('rename は windowId を保ち、既存名・存在しない対象は失敗する', () => {
  const layout = new Layout();
  layout.createWorkspace('a');
  layout.createWorkspace('b');
  const { windowId } = layout.createWindow('a', 'w');
  layout.renameWorkspace('a', 'c');
  assert.deepEqual(layout.windowIds('c'), [windowId]);
  assert.deepEqual(layout.windowIds('a'), []);
  expectFailure(() => layout.renameWorkspace('c', 'b'), ErrorCode.AlreadyExists);
  expectFailure(() => layout.renameWorkspace('zz', 'q'), ErrorCode.WorkspaceNotFound);
  layout.renameWindow(windowId, 'x');
  assert.deepEqual(layout.windowRef(windowId), { workspace: 'c', window: { id: windowId, name: 'x' } });
  expectFailure(() => layout.renameWindow(`w_${ulid()}`, 'x'), ErrorCode.WindowNotFound);
});

test('rename は以前に返した list の結果を書き換えない', () => {
  const layout = new Layout();
  layout.createWorkspace('a');
  const before = layout.list();
  layout.renameWorkspace('a', 'b');
  assert.equal(before[0]!.name, 'a');
});

test('close は削除した windowId 群を返し、window の close は他を残す', () => {
  const layout = new Layout();
  layout.createWorkspace('a');
  const w1 = layout.createWindow('a', 'x');
  const w2 = layout.createWindow('a', 'y');
  layout.closeWindow(w1.windowId);
  expectFailure(() => layout.windowRef(w1.windowId), ErrorCode.WindowNotFound);
  assert.deepEqual(layout.closeWorkspace('a'), [w2.windowId]);
  expectFailure(() => layout.closeWorkspace('a'), ErrorCode.WorkspaceNotFound);
  expectFailure(() => layout.closeWindow(w2.windowId), ErrorCode.WindowNotFound);
});

test('resolveWindow: 省略時は default を作り、2 回目以降は再利用する', () => {
  const layout = new Layout();
  const first = layout.resolveWindow(undefined);
  assert.equal(first.workspace, 'default');
  assert.equal(first.window.name, 'default');
  assert.equal(first.createdWorkspace, true);
  assert.equal(first.createdWindow, true);
  const second = layout.resolveWindow(undefined);
  assert.deepEqual(second.window, first.window);
  assert.equal(second.createdWorkspace || second.createdWindow, false);
  const explicit = layout.resolveWindow(first.window.id);
  assert.equal(explicit.workspace, 'default');
  expectFailure(() => layout.resolveWindow(`w_${ulid()}`), ErrorCode.WindowNotFound);
});

test('resolveWindow: default workspace があって window が無ければ window だけ作る', () => {
  const layout = new Layout();
  layout.createWorkspace('default');
  const r = layout.resolveWindow(undefined);
  assert.equal(r.createdWorkspace, false);
  assert.equal(r.createdWindow, true);
});

test('toPersisted / restore で往復し、list は workspace 付きの window を返す', () => {
  const layout = new Layout();
  layout.createWorkspace('a');
  const w = layout.createWindow('a', 'x');
  const restored = new Layout();
  restored.restore(layout.toPersisted());
  assert.deepEqual(restored.list(), [{ name: 'a', windows: [{ windowId: w.windowId, name: 'x', workspace: 'a' }] }]);
});

test('closing 中の workspace / window には作成・解決・rename・二重 close を拒否し、cancel で戻る', () => {
  const layout = new Layout();
  layout.createWorkspace('a');
  const w = layout.createWindow('a', 'x');
  assert.deepEqual(layout.beginCloseWorkspace('a'), [w.windowId]);
  assert.throws(() => layout.createWindow('a', 'y'), /closing/);
  assert.throws(() => layout.resolveWindow(w.windowId), /closing/);
  assert.throws(() => layout.renameWorkspace('a', 'b'), /closing/);
  assert.throws(() => layout.beginCloseWorkspace('a'), /closing/);
  assert.throws(() => layout.beginCloseWindow(w.windowId), /closing/);
  expectFailure(() => layout.createWorkspace('a'), ErrorCode.AlreadyExists);
  layout.cancelCloseWorkspace('a');
  assert.doesNotThrow(() => layout.createWindow('a', 'y'));
  assert.doesNotThrow(() => layout.resolveWindow(w.windowId));
});

test('closing の window は既定 window として使い回さず、close 完了で印も消える', () => {
  const layout = new Layout();
  const def = layout.resolveWindow(undefined);
  layout.beginCloseWindow(def.window.id);
  const next = layout.resolveWindow(undefined);
  assert.notEqual(next.window.id, def.window.id);
  assert.equal(next.createdWindow, true);
  layout.closeWindow(def.window.id);
  layout.beginCloseWorkspace('default');
  layout.closeWorkspace('default');
  layout.createWorkspace('default');
  assert.doesNotThrow(() => layout.createWindow('default', 'again'));
});
