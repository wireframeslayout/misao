import assert from 'node:assert/strict';
import { test } from 'node:test';
import { run } from '../../src/run.js';
import { createTestIo } from '../helpers/io.js';
import type { TestIo } from '../helpers/io.js';
import { openTestPane, startTestDaemon, waitFor } from '../helpers/daemon.js';
import type { TestDaemon } from '../helpers/daemon.js';

/** 追い続けるコマンドを起動する。stop() で SIGINT を送り、終了コードを返す。 */
function startFollow(daemon: TestDaemon, argv: string[]): { io: TestIo; stop(): Promise<number> } {
  const io = createTestIo({ env: daemon.env, homeDir: daemon.dir });
  const done = run(argv, io);
  return {
    io,
    async stop() {
      io.emitSignal('SIGINT');
      return done;
    },
  };
}

const TWO_LINES = ['sh', '-c', 'echo first; sleep 0.5; echo second; sleep 60'];

test('tail: 保持分を再生してから追い、SIGINT で 0 になる', async () => {
  const daemon = await startTestDaemon();
  try {
    const paneId = await openTestPane(daemon.client, TWO_LINES, { labels: { name: 'tail-me' } });
    const t = startFollow(daemon, ['tail', 'tail-me']);
    await waitFor(() => t.io.out().includes('second'));
    assert.equal(await t.stop(), 0);
    assert.equal(t.io.out(), 'first\nsecond\n');
    assert.equal(t.io.err(), '');
    await daemon.client.request('pane.close', { paneId });
  } finally {
    await daemon.stop();
  }
});

test('tail --since / --json: 指定した seq より後だけを NDJSON で出す', async () => {
  const daemon = await startTestDaemon();
  try {
    const paneId = await openTestPane(daemon.client, TWO_LINES);
    const t = startFollow(daemon, ['tail', paneId, '--since', '1', '--json']);
    await waitFor(() => t.io.out().includes('second'));
    assert.equal(await t.stop(), 0);
    const lines = t.io.out().trim().split('\n').map((l) => JSON.parse(l) as { seq: number; text: string; paneId: string });
    assert.deepEqual(lines.map((l) => [l.seq, l.text, l.paneId]), [[2, 'second', paneId]]);
    assert.equal(t.io.err(), '', 'gap でなければ警告しない');
  } finally {
    await daemon.stop();
  }
});

test('tail: 保持範囲より古い --since は gap を stderr に警告し、--since なしでは警告しない', async () => {
  const daemon = await startTestDaemon({ rings: { linesBytes: 30 } });
  try {
    const manyLines = ['sh', '-c', 'for i in 1 2 3 4 5 6; do echo "line-number-$i"; done; sleep 60'];
    const paneId = await openTestPane(daemon.client, manyLines);
    await waitFor(async () => (await daemon.client.request('pane.screen', { paneId })).text.includes('line-number-6'));

    const withSince = startFollow(daemon, ['tail', paneId, '--since', '1']);
    await waitFor(() => withSince.io.out().includes('line-number-6'));
    assert.equal(await withSince.stop(), 0);
    assert.match(withSince.io.err(), /警告: 取りこぼし/);

    const without = startFollow(daemon, ['tail', paneId]);
    await waitFor(() => without.io.out().includes('line-number-6'));
    assert.equal(await without.stop(), 0);
    assert.equal(without.io.err(), '');
  } finally {
    await daemon.stop();
  }
});

test('tail: 対象がなければ 1、--since が不正なら 2', async () => {
  const daemon = await startTestDaemon();
  try {
    const io = createTestIo({ env: daemon.env, homeDir: daemon.dir });
    assert.equal(await run(['tail', 'nothing'], io), 1);
    assert.equal(await run(['tail', 'nothing', '--since', 'x'], createTestIo({ env: daemon.env, homeDir: daemon.dir })), 2);
  } finally {
    await daemon.stop();
  }
});

test('events: ライブのイベントを NDJSON で追い、--pane で絞れる', async () => {
  const daemon = await startTestDaemon();
  try {
    const a = await openTestPane(daemon.client, ['sh', '-c', 'sleep 60'], { labels: { name: 'ev-a' } });
    const b = await openTestPane(daemon.client, ['sh', '-c', 'sleep 60'], { labels: { name: 'ev-b' } });
    const all = startFollow(daemon, ['events', '--json']);
    const only = startFollow(daemon, ['events', '--json', '--pane', 'ev-a']);
    await new Promise((r) => setTimeout(r, 200)); // 購読の確立を待つ
    await daemon.client.request('pane.set_label', { paneId: b, set: { who: 'b' } });
    await daemon.client.request('pane.set_label', { paneId: a, set: { who: 'a' } });
    const parse = (io: TestIo): Array<{ type: string; paneId?: string; seq: number }> =>
      io.out().trim().split('\n').filter(Boolean).map((l) => JSON.parse(l) as { type: string; paneId?: string; seq: number });
    await waitFor(() => parse(all.io).length >= 2 && parse(only.io).length >= 1);
    assert.equal(await all.stop(), 0);
    assert.equal(await only.stop(), 0);
    assert.deepEqual(parse(all.io).map((e) => [e.type, e.paneId]), [['pane.label', b], ['pane.label', a]]);
    assert.deepEqual(parse(only.io).map((e) => [e.type, e.paneId]), [['pane.label', a]]);
    await daemon.client.request('pane.close', { paneId: a });
    await daemon.client.request('pane.close', { paneId: b });
  } finally {
    await daemon.stop();
  }
});

test('events --since 0: 保持しているイベントを再生する (人向けの 1 行形式)', async () => {
  const daemon = await startTestDaemon();
  try {
    const e = startFollow(daemon, ['events', '--since', '0']);
    await waitFor(() => e.io.out().includes('daemon.started'));
    assert.equal(await e.stop(), 0);
    assert.match(e.io.out(), /^1  \d{4}-\d{2}-\d{2}T[\d:.]+Z  daemon\.started  -  \{.*"pid":\d+/m);
  } finally {
    await daemon.stop();
  }
});
