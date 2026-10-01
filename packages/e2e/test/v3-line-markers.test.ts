import assert from 'node:assert/strict';
import * as path from 'node:path';
import { after, before, describe, test } from 'node:test';
import type { GapInfo, MisaoClient } from '@misao/sdk';
import { MARKER_RE, fakeAgentCmd, openPane, readAgentSummary, waitForExit } from './helpers/agents.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { scale } from './helpers/scale.js';
import { sleep, waitFor } from './helpers/wait.js';

const NORMAL_ROUNDS = scale(200, 1000);
const BURST_ROUNDS = scale(100, 1000);
const EXIT_TIMEOUT_MS = scale(120_000, 15 * 60_000);
/** 遅い購読者のテスト用。burst 1 ラウンド (約 0.5 MiB) は収まり、数ラウンドで溢れる容量。 */
const SMALL_LINES_RING_BYTES = 1024 * 1024;

interface Env {
  daemon: DaemonProcess;
  /** 購読者とは別の、制御用クライアント (終了待ち・head 取得)。 */
  client: MisaoClient;
  epoch: string;
}

async function startEnv(config: Parameters<typeof startDaemonProcess>[0]): Promise<Env> {
  const daemon = await startDaemonProcess(config);
  const client = await daemon.connect();
  return { daemon, client, epoch: (await client.request('server.info', {})).epoch };
}

let env: Env; // 既定の設定
let small: Env; // 行リングを小さくしたデーモン

before(async () => {
  env = await startEnv({});
  small = await startEnv({ rings: { linesBytes: SMALL_LINES_RING_BYTES } });
});
after(async () => {
  await env.daemon.stop();
  await small.daemon.stop();
});

/** 購読の結果。subscriber は呼び出し側が close する。 */
interface Subscription {
  sub: MisaoClient;
  paneId: string;
  found: string[];
  seqs: number[];
  gaps: GapInfo[];
  /** connected 以外に遷移した回数の内訳 (= 切断・再接続)。 */
  connectionChanges: string[];
}

/** fake-agent markers を流す pane を SDK の行購読 (先頭から) で受ける。onFirstLine は最初の行を受けた時点で同期的に呼ぶ。 */
async function startMarkers(e: Env, id: string, rounds: number, onFirstLine?: () => void): Promise<Subscription & { outFile: string }> {
  const outFile = path.join(e.daemon.dir, `${id}.summary`);
  const sub = await e.daemon.connect();
  const result: Subscription = { sub, paneId: '', found: [], seqs: [], gaps: [], connectionChanges: [] };
  sub.onGap((gap) => result.gaps.push(gap));
  sub.onStateChange((state) => state.status !== 'connected' && result.connectionChanges.push(state.status));
  result.paneId = await openPane(e.client, fakeAgentCmd('markers', '--count', String(rounds), '--id', id, '--out', outFile, '--burst'));
  await sub.subscribeLines(
    result.paneId,
    (line) => {
      if (result.seqs.length === 0) onFirstLine?.();
      result.seqs.push(line.seq);
      for (const match of line.text.matchAll(MARKER_RE)) result.found.push(match[0]);
    },
    { since: 0, epoch: e.epoch },
  );
  return { ...result, outFile };
}

/** pane の終了と、購読者が head まで届くのを待つ (どちらも購読者とは別クライアントで確認する)。 */
async function waitForHead(e: Env, s: Subscription): Promise<void> {
  const info = await waitForExit(e.client, s.paneId, EXIT_TIMEOUT_MS);
  assert.equal(info.exitCode, 0);
  const { head } = await e.client.request('pane.subscribe_lines', { paneId: s.paneId });
  await waitFor(() => s.seqs.at(-1) === head, 'the subscriber to reach the head', { timeoutMs: EXIT_TIMEOUT_MS, stepMs: 200 });
}

/** 誤検出と重複がない。 */
function assertNoFalseMarkers(s: Subscription, id: string, outFile: string): { expected: string[]; unique: Set<string> } {
  const expected = readAgentSummary(outFile).nonces.map((nonce) => `AZITO_DONE_${id}_${nonce}`);
  const unique = new Set(s.found);
  assert.deepEqual([...unique].filter((marker) => !expected.includes(marker)), [], '誤検出');
  assert.equal(s.found.length, unique.size, '重複');
  assert.ok(s.seqs.every((seq, i) => i === 0 || seq > s.seqs[i - 1]!), 'seq が単調増加 (重複・逆行がない)');
  return { expected, unique };
}

/** seq の不連続 (先頭が 1 でない場合を含む) の数。 */
function countBreaks(seqs: number[]): number {
  return seqs.filter((seq, i) => seq !== (seqs[i - 1] ?? 0) + 1).length;
}

/**
 * 先頭から購読した行に含まれるマーカーを、fake-agent の申告 (summary) と突き合わせる。
 * 通常でも burst でも、gap・切断・取り逃し・seq の欠けはいずれも起きない。
 */
async function runMarkers(id: string, rounds: number): Promise<void> {
  const s = await startMarkers(env, id, rounds);
  await waitForHead(env, s);
  const { expected, unique } = assertNoFalseMarkers(s, id, s.outFile);
  assert.deepEqual([s.gaps, s.connectionChanges], [[], []], 'gap も切断も起きない');
  assert.equal(countBreaks(s.seqs), 0, 'seq が 1 から連続する');
  assert.deepEqual(expected.filter((marker) => !unique.has(marker)), [], '取り逃したマーカー');
  s.sub.close();
  await env.client.request('pane.close', { paneId: s.paneId });
}

/** ハンドラを同期的に止める (イベントループごと止まるので、ソケットを読まない遅い購読者になる)。 */
function blockThread(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

describe('v3: 行ストリームのマーカー検出', () => {
  test('通常出力: 取り逃し・誤検出・重複がなく seq が連続する', { timeout: EXIT_TIMEOUT_MS + 60_000 }, async () => {
    await runMarkers('N1', NORMAL_ROUNDS);
  });

  test('burst 出力 (大量の行): gap・切断・取り逃しがなく seq が連続する', { timeout: EXIT_TIMEOUT_MS + 60_000 }, async () => {
    await runMarkers('B1', BURST_ROUNDS);
  });

  test('遅い購読者 (リングに収まる停止): 切断も gap もなく、全件が連続して届く', { timeout: 120_000 }, async () => {
    const s = await startMarkers(small, 'S1', 1, () => blockThread(1500)); // burst 1 ラウンド (約 0.5 MiB) はリングに収まる
    await waitForHead(small, s);
    assert.deepEqual([s.gaps, s.connectionChanges], [[], []]);
    assert.equal(countBreaks(s.seqs), 0);
    const { expected, unique } = assertNoFalseMarkers(s, 'S1', s.outFile);
    assert.deepEqual(expected.filter((marker) => !unique.has(marker)), []);
    s.sub.close();
    await small.client.request('pane.close', { paneId: s.paneId });
  });

  test('遅い購読者 (リングを超える停止): リングに追い越されたときだけ切断され、gap(truncated) で知らされる', { timeout: 120_000 }, async () => {
    const s = await startMarkers(small, 'S2', 20, () => blockThread(6000)); // 約 10 MiB > リング + 送信キュー
    await waitForHead(small, s);
    assert.ok(s.gaps.length >= 1, '追い越された購読者は gap になる');
    assert.ok(s.connectionChanges.length >= 1, '追い越された購読者は切断される');
    assert.deepEqual(s.gaps.map((gap) => gap.reason).filter((reason) => reason !== 'truncated'), [], 'gap の理由は truncated だけ');
    assertNoFalseMarkers(s, 'S2', s.outFile);
    // seq の不連続は、gap で知らされた回数を超えてはならない (黙って失わない)。
    assert.ok(countBreaks(s.seqs) <= s.gaps.length, `seq の不連続 ${countBreaks(s.seqs)} 件が gap ${s.gaps.length} 件を超えている`);
    s.sub.close();
    await small.client.request('pane.close', { paneId: s.paneId });
  });

  test('再購読: 取りこぼした範囲を gap として知らされ、保持範囲から再生される', { timeout: 120_000 }, async () => {
    const { daemon, client, epoch } = small;
    const outFile = path.join(daemon.dir, 'resume.summary');
    const paneId = await openPane(client, fakeAgentCmd('markers', '--count', '3', '--id', 'R', '--out', outFile, '--burst')); // 約 1.5 MiB > リング
    const first = await daemon.connect();
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
    const resumed = await daemon.connect();
    resumed.onGap((gap) => gaps.push(gap));
    await resumed.subscribeLines(paneId, (line) => replayed.push(line.seq), { since: stoppedAt, epoch });
    await waitFor(() => replayed.at(-1) === head, 'the replay to reach the head', { timeoutMs: 10_000 });
    assert.deepEqual(gaps.map((gap) => gap.reason), ['truncated']);
    assert.ok(replayed[0]! > stoppedAt + 1, '古い行は再生されない');

    // 対照: 直近 (head) からの再購読は gap にならず、head より先からは gap になる
    const atHead = await daemon.connect();
    const atHeadGaps: GapInfo[] = [];
    atHead.onGap((gap) => atHeadGaps.push(gap));
    await atHead.subscribeLines(paneId, () => undefined, { since: head, epoch });
    assert.deepEqual(atHeadGaps, []);
    const beyond = await daemon.connect();
    const beyondGaps: GapInfo[] = [];
    beyond.onGap((gap) => beyondGaps.push(gap));
    await beyond.subscribeLines(paneId, () => undefined, { since: head + 1000, epoch });
    assert.deepEqual(beyondGaps.map((gap) => gap.reason), ['truncated']);
    await client.request('pane.close', { paneId });
  });
});
