import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { loadPersistedState, savePersistedState } from '../src/persistence.js';
import type { PersistedState } from '../src/persistence.js';
import { newPaneId, newWindowId } from '../src/ulid.js';

function withDir(fn: (dir: string) => void): void {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-persist-'));
  try {
    fn(dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

function sampleState(): PersistedState {
  const windowId = newWindowId();
  return {
    version: 1,
    workspaces: [{ name: 'default', windows: [{ id: windowId, name: 'default' }] }],
    panes: [
      { paneId: newPaneId(), windowId, cmd: ['sh', '-c', 'x'], cwd: '/tmp', env: { A: '1' }, labels: { owner: 'me' }, cols: 80, rows: 24 },
    ],
  };
}

test('保存したものを読み戻せる', () => {
  withDir((dir) => {
    const file = path.join(dir, 'persistence.json');
    const state = sampleState();
    savePersistedState(file, state);
    assert.deepEqual(loadPersistedState(file), state);
  });
});

test('ファイルが無ければ空の状態', () => {
  withDir((dir) => {
    assert.deepEqual(loadPersistedState(path.join(dir, 'none.json')), { version: 1, workspaces: [], panes: [] });
  });
});

test('壊れた JSON・スキーマ違反・未知の version は path を含む例外', () => {
  withDir((dir) => {
    const file = path.join(dir, 'persistence.json');
    const bad: string[] = ['{not json', '{"version":1}', JSON.stringify({ ...sampleState(), version: 2 })];
    bad.push(JSON.stringify({ ...sampleState(), extra: true }));
    for (const text of bad) {
      fs.writeFileSync(file, text);
      assert.throws(() => loadPersistedState(file), (e: unknown) => e instanceof Error && e.message.includes(file), text);
    }
  });
});

test('存在しない window を指す pane や重複した ID は拒否する', () => {
  withDir((dir) => {
    const file = path.join(dir, 'persistence.json');
    const orphan = sampleState();
    orphan.panes[0] = { ...orphan.panes[0]!, windowId: newWindowId() };
    const dup = sampleState();
    dup.panes.push(dup.panes[0]!);
    for (const state of [orphan, dup]) {
      fs.writeFileSync(file, JSON.stringify(state));
      assert.throws(() => loadPersistedState(file), /invalid persistence file/);
    }
  });
});

test('mode 0600 で書き、一時ファイルを残さない', () => {
  withDir((dir) => {
    const file = path.join(dir, 'persistence.json');
    savePersistedState(file, sampleState());
    savePersistedState(file, sampleState());
    assert.equal(fs.statSync(file).mode & 0o777, 0o600);
    assert.deepEqual(fs.readdirSync(dir), ['persistence.json']);
  });
});

test('書き込みに失敗したら例外を伝播し、一時ファイルを残さない', () => {
  withDir((dir) => {
    const file = path.join(dir, 'persistence.json');
    fs.mkdirSync(file); // rename 先がディレクトリ
    assert.throws(() => savePersistedState(file, sampleState()));
    assert.deepEqual(fs.readdirSync(dir), ['persistence.json']);
  });
});
