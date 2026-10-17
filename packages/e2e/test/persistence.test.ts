import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';
import { ErrorCode } from '@misao/protocol';
import { MisaoRpcError } from '@misao/sdk';
import { paneInfo } from './helpers/agents.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonImpl } from './helpers/daemon-process.js';
import { IS_WINDOWS, sleepCmd } from './helpers/platform.js';
import { waitFor } from './helpers/wait.js';

/** 実装をまたぐ確認は、Rust 版（`MISAO_DAEMON_BIN`）を指定したときだけ行う。 */
const CROSS =
  (process.env['MISAO_DAEMON_BIN'] ?? '') === '' ? { skip: 'MISAO_DAEMON_BIN を指定したときだけ（Rust 版が要る）' } : IS_WINDOWS ? { skip: 'Node 版のデーモンは Windows の CI では動かさない（Windows (experimental) が見る）' } : {};

/** 1 回目のデーモンで状態を作って止め、2 回目のデーモン（同じディレクトリ）で復元を確かめる。 */
async function restartScenario(first: DaemonImpl | undefined, second: DaemonImpl | undefined): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-e2e-persist-'));
  try {
    const one = await startDaemonProcess({}, { dir, ...(first && { impl: first }) });
    const client = await one.connect();
    await client.request('workspace.create', { name: 'ws' });
    const { windowId } = await client.request('window.create', { workspace: 'ws', name: 'main' });
    const { paneId } = await client.request('pane.open', {
      cmd: sleepCmd(),
      windowId,
      cols: 90,
      rows: 33,
      env: { VISIBLE: 'yes' },
      ephemeralEnv: { SECRET_TOKEN: 'must-not-persist' },
      labels: { task: '7' },
    });
    await client.request('pane.set_label', { paneId, set: { owner: 'x' } });
    await client.request('pane.resize', { paneId, cols: 70, rows: 20 });
    // 遅らせて保存するサイズも、止める前に保存される
    await one.stop();
    const saved = fs.readFileSync(path.join(dir, 'persistence.json'), 'utf8');
    assert.ok(!saved.includes('must-not-persist'), 'ephemeralEnv は保存しない');
    assert.ok(saved.includes('VISIBLE'), 'env は保存する');

    const two = await startDaemonProcess({}, { dir, ...(second && { impl: second }) });
    try {
      const again = await two.connect();
      const info = await paneInfo(again, paneId);
      assert.equal(info.processState, 'stopped');
      assert.deepEqual([info.pid, info.exitCode, info.signal, info.title], [null, null, null, '']);
      assert.deepEqual(info.labels, { task: '7', owner: 'x' });
      assert.deepEqual([info.cols, info.rows], [70, 20]);
      assert.deepEqual([info.workspace, info.window.name], ['ws', 'main']);
      assert.equal((await again.request('pane.list', { filter: { state: 'stopped' } })).length, 1);
      await assert.rejects(again.request('pane.write', { paneId, data: 'x' }), (e) => e instanceof MisaoRpcError && e.code === ErrorCode.PaneExited);
      await again.request('pane.set_label', { paneId, set: { after: 'restart' } });
      await again.request('pane.close', { paneId });
      await assert.rejects(paneInfo(again, paneId), (e) => e instanceof MisaoRpcError && e.code === ErrorCode.PaneNotFound);
      await waitFor(() => !fs.readFileSync(path.join(dir, 'persistence.json'), 'utf8').includes(paneId), 'the record to be removed');
    } finally {
      await two.stop();
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test('再起動後、ペインは stopped として復元され、ラベルの編集と close ができる', { timeout: 60_000 }, async () => {
  await restartScenario(undefined, undefined);
});

test('Node 版が保存した状態を Rust 版が復元する', { timeout: 60_000, ...CROSS }, async () => {
  await restartScenario('node', 'rust');
});

test('Rust 版が保存した状態を Node 版が復元する', { timeout: 60_000, ...CROSS }, async () => {
  await restartScenario('rust', 'node');
});

test('壊れた状態のファイルでは、どちらの実装も起動を止める', { timeout: 60_000 }, async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-e2e-persist-'));
  try {
    fs.writeFileSync(path.join(dir, 'persistence.json'), '{"version": 2, "workspaces": [], "panes": []}');
    await assert.rejects(startDaemonProcess({}, { dir }), /invalid persistence file/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
