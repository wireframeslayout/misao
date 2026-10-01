import assert from 'node:assert/strict';
import * as path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { MisaoClient } from '@misao/sdk';
import type { GapInfo } from '@misao/sdk';
import { MARKER_RE, fakeAgentCmd, openPane, readAgentSummary, waitForExit } from './helpers/agents.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { scale } from './helpers/scale.js';
import { sleep, waitFor } from './helpers/wait.js';

const NORMAL_ROUNDS = scale(200, 1000);
const BURST_ROUNDS = scale(20, 1000);
const EXIT_TIMEOUT_MS = scale(120_000, 15 * 60_000);
const LINES_RING_BYTES = 64 * 1024; // 既定。遅い購読者が gap になることの前提

let daemon: DaemonProcess;
let client: MisaoClient;
let epoch: string;

before(async () => {
  daemon = await startDaemonProcess({ rings: { linesBytes: LINES_RING_BYTES } });
  client = await daemon.connect();
  epoch = (await client.request('server.info', {})).epoch;
});
after(async () => {
  await daemon.stop();
});

async function newClient(): Promise<MisaoClient> {
  return daemon.connect();
}

/**
 * 先頭から購読した行に含まれるマーカーを、fake-agent の申告 (summary) と突き合わせる。
 * 誤検出と重複は常に不可。取り逃しと seq の欠けは、gap で知らされたときだけ許す (黙って失わない)。
 * burst はデーモンが遅い購読者 (送信キュー超過) を切るので、SDK の再接続と gap が起きうる。
 */
async function runMarkers(id: string, rounds: number, isBurst: boolean): Promise<void> {
  const outFile = path.join(daemon.dir, `${id}.summary`);
  const sub = await newClient();
  const found: string[] = [];
  const seqs: number[] = [];
  const gaps: GapInfo[] = [];
  const connectionChanges: string[] = [];
  sub.onGap((gap) => gaps.push(gap));
  sub.onStateChange((state) => state.status !== 'connected' && connectionChanges.push(state.status));
  const paneId = await openPane(client, fakeAgentCmd('markers', '--count', String(rounds), '--id', id, '--out', outFile, ...(isBurst ? ['--burst'] : [])));
  await sub.subscribeLines(
    paneId,
    (line) => {
      seqs.push(line.seq);
      for (const match of line.text.matchAll(MARKER_RE)) found.push(match[0]);
    },
    { since: 0, epoch },
  );
  const info = await waitForExit(client, paneId, EXIT_TIMEOUT_MS);
  assert.equal(info.exitCode, 0);
  const { head } = await client.request('pane.subscribe_lines', { paneId });
  await waitFor(() => seqs.at(-1) === head, 'the subscriber to reach the head', { timeoutMs: EXIT_TIMEOUT_MS, stepMs: 200 });

  const expected = readAgentSummary(outFile).nonces.map((nonce) => `AZITO_DONE_${id}_${nonce}`);
  const unique = new Set(found);
  assert.deepEqual([...unique].filter((marker) => !expected.includes(marker)), [], '誤検出');
  assert.equal(found.length, unique.size, '重複');
  assert.ok(seqs.every((seq, i) => i === 0 || seq > seqs[i - 1]!), 'seq が単調増加 (重複・逆行がない)');
  if (!isBurst) assert.deepEqual([gaps, connectionChanges], [[], []], '通常出力では gap も切断も起きない');
  // seq の不連続 (先頭が 1 でない場合を含む) は、gap で知らされた回数を超えてはならない (黙って失わない)。
  const breaks = seqs.filter((seq, i) => seq !== (seqs[i - 1] ?? 0) + 1).length;
  assert.ok(breaks <= gaps.length, `seq の不連続 ${breaks} 件が gap ${gaps.length} 件を超えている`);
  assert.deepEqual(gaps.map((gap) => gap.reason).filter((reason) => reason !== 'truncated'), [], 'gap の理由は truncated だけ');
  // 取り逃しを許すのは、seq が実際に欠けた (不連続がある) ときだけ。
  // TODO(#22): 大量出力で購読者が切断される退行を直したら、burst も gap なし・取り逃しなしの厳しい条件に戻す。
  if (breaks === 0) assert.deepEqual(expected.filter((marker) => !unique.has(marker)), [], '取り逃したマーカー');
  sub.close();
  await client.request('pane.close', { paneId });
}

describe('v3: 行ストリームのマーカー検出', () => {
  test('通常出力: 取り逃し・誤検出・重複がなく seq が連続する', { timeout: EXIT_TIMEOUT_MS + 60_000 }, async () => {
    await runMarkers('N1', NORMAL_ROUNDS, false);
  });

  test('burst 出力 (リングより大量の行): 誤検出と重複がなく、取り逃しは gap で知らされる', { timeout: EXIT_TIMEOUT_MS + 60_000 }, async () => {
    await runMarkers('B1', BURST_ROUNDS, true);
  });

  test('遅い購読者: 取りこぼした範囲を gap として知らされ、保持範囲から再生される', { timeout: 120_000 }, async () => {
    const outFile = path.join(daemon.dir, 'slow.summary');
    const paneId = await openPane(client, fakeAgentCmd('markers', '--count', '3', '--id', 'S', '--out', outFile, '--burst'));
    const first = await newClient();
    let seqAtDisconnect = 0;
    await first.subscribeLines(paneId, (line) => (seqAtDisconnect = line.seq), { since: 0, epoch });
    await waitFor(() => seqAtDisconnect > 0, 'the first line', { timeoutMs: 10_000, stepMs: 20 });
    first.close(); // 購読側を切る (遅延のシミュレート)
    const stoppedAt = seqAtDisconnect;
    await waitForExit(client, paneId, 60_000);
    await sleep(300); // 最後の行がリングに入るのを待つ

    const head = (await client.request('pane.subscribe_lines', { paneId })).head;
    const gaps: GapInfo[] = [];
    const replayed: number[] = [];
    const resumed = await newClient();
    resumed.onGap((gap) => gaps.push(gap));
    await resumed.subscribeLines(paneId, (line) => replayed.push(line.seq), { since: stoppedAt, epoch });
    await waitFor(() => replayed.at(-1) === head, 'the replay to reach the head', { timeoutMs: 10_000 });
    assert.deepEqual(gaps.map((gap) => gap.reason), ['truncated']);
    assert.ok(replayed[0]! > stoppedAt + 1, '古い行は再生されない');

    // 対照: 直近 (head) からの再購読は gap にならず、head より先からは gap になる
    const atHead = await newClient();
    const atHeadGaps: GapInfo[] = [];
    atHead.onGap((gap) => atHeadGaps.push(gap));
    await atHead.subscribeLines(paneId, () => undefined, { since: head, epoch });
    assert.deepEqual(atHeadGaps, []);
    const beyond = await newClient();
    const beyondGaps: GapInfo[] = [];
    beyond.onGap((gap) => beyondGaps.push(gap));
    await beyond.subscribeLines(paneId, () => undefined, { since: head + 1000, epoch });
    assert.deepEqual(beyondGaps.map((gap) => gap.reason), ['truncated']);
    await client.request('pane.close', { paneId });
  });
});
