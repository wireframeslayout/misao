// v3: 行ストリームのマーカー検出 (取り逃し / 誤検出 / 重複 / seq 連続性 / gap)。結果は verify/results/v3.json
import * as fs from 'node:fs';
import * as path from 'node:path';
import { MisaoClient } from '../src/client/MisaoClient.js';
import { open, paneInfo, sleep, startDaemon, waitFor, writeResult, ROOT } from './lib.js';

const RE = /AZITO_DONE_[A-Za-z0-9]+_[A-Za-z0-9]+/g;
const BURST_ROUNDS = Number(process.env.V3_BURST_ROUNDS ?? 1000);
const result: Record<string, unknown> = { startedAt: new Date().toISOString(), cases: {} };
const cases = result.cases as Record<string, unknown>;

async function runMarkers(socket: string, c: MisaoClient, dir: string, id: string, count: number, burst: boolean) {
  const outFile = path.join(dir, `${id}.summary`);
  const sub = await MisaoClient.connect(socket);
  const found: string[] = [];
  const seqs: number[] = [];
  let closedEarly = false;
  sub.on('close', () => (closedEarly = true));
  sub.onNotification((n) => {
    if (n.method !== 'pane.line') return;
    seqs.push(n.params.seq);
    for (const m of (n.params.text as string).matchAll(RE)) found.push(m[0]);
  });
  const t0 = Date.now();
  const paneId = await open(c, [
    process.execPath, path.join(ROOT, 'dist/fixtures/fake-agent.js'), 'markers', '--count', String(count), '--id', id, '--out', outFile,
    ...(burst ? ['--burst'] : []),
  ]);
  const res = await sub.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since: 0 });
  const exited = await waitFor(async () => (await paneInfo(c, paneId)).state === 'exited', 15 * 60_000, 500);
  await sleep(1500);
  const info = await paneInfo(c, paneId);
  const summary = fs.existsSync(outFile) ? (JSON.parse(fs.readFileSync(outFile, 'utf8').replace(/^FAKE_SUMMARY /, '')) as { emitted: number; nonces: string[] }) : undefined;
  const expected = new Set((summary?.nonces ?? []).map((n) => `AZITO_DONE_${id}_${n}`));
  const counts = new Map<string, number>();
  for (const f of found) counts.set(f, (counts.get(f) ?? 0) + 1);
  const missed = [...expected].filter((e) => !counts.has(e));
  const falsePositives = [...counts.keys()].filter((k) => !expected.has(k));
  const duplicates = [...counts.entries()].filter(([k, v]) => v > 1 && expected.has(k)).map(([k, v]) => ({ marker: k, count: v }));
  let seqBreaks = 0;
  for (let i = 1; i < seqs.length; i++) if (seqs[i] !== seqs[i - 1]! + 1) seqBreaks++;
  const out = {
    rounds: count, burst, elapsedSec: Math.round((Date.now() - t0) / 1000), paneExited: exited, exitCode: info.exitCode,
    summaryFound: !!summary, expected: expected.size, detected: found.length, missed: missed.length, missedSample: missed.slice(0, 5),
    falsePositives: falsePositives.length, falsePositiveSample: falsePositives.slice(0, 5),
    duplicates: duplicates.length, duplicateSample: duplicates.slice(0, 5),
    linesReceived: seqs.length, firstSeq: seqs[0], lastSeq: seqs[seqs.length - 1], seqBreaks,
    gapOnSubscribe: res.gap, headAtSubscribe: res.head, subscriberDisconnected: closedEarly,
    pass: exited && !!summary && missed.length === 0 && falsePositives.length === 0 && duplicates.length === 0 && !closedEarly,
  };
  sub.close();
  await c.request('pane.close', { paneId });
  return out;
}

async function slowSubscriber(socket: string, c: MisaoClient, dir: string) {
  const outFile = path.join(dir, 'slow.summary');
  const sub = await MisaoClient.connect(socket);
  let lastSeq = 0;
  sub.onNotification((n) => {
    if (n.method === 'pane.line') lastSeq = n.params.seq;
  });
  const paneId = await open(c, [
    process.execPath, path.join(ROOT, 'dist/fixtures/fake-agent.js'), 'markers', '--count', '3', '--id', 'S', '--out', outFile, '--burst',
  ]);
  await sub.request('pane.subscribe_lines', { paneId, since: 0 });
  await waitFor(() => lastSeq > 0, 10000, 20);
  const seqAtDisconnect = lastSeq;
  sub.close(); // 購読側を切る (遅延をシミュレート)
  await waitFor(async () => (await paneInfo(c, paneId)).state === 'exited', 60000, 200);
  await sleep(500);
  const sub2 = await MisaoClient.connect(socket);
  const replayed: number[] = [];
  sub2.onNotification((n) => n.method === 'pane.line' && replayed.push(n.params.seq));
  const r1 = await sub2.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since: seqAtDisconnect });
  await sleep(500);
  // 対照: 直近 seq からの再購読は gap:false
  const sub3 = await MisaoClient.connect(socket);
  const r2 = await sub3.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since: r1.head });
  const r3 = await sub3.request<{ gap: boolean; head: number }>('pane.subscribe_lines', { paneId, since: r1.head + 1000 });
  const out = {
    seqAtDisconnect, headAfter: r1.head, gapReported: r1.gap, firstReplayedSeq: replayed[0], replayedCount: replayed.length,
    expectGapTrue: r1.head - seqAtDisconnect > 64 * 1024 / 30,
    controlSinceHeadGap: r2.gap, controlSinceBeyondHeadGap: r3.gap,
    pass: r1.gap === true && (replayed[0] ?? 0) > seqAtDisconnect + 1 && r2.gap === false && r3.gap === true,
  };
  sub2.close();
  sub3.close();
  await c.request('pane.close', { paneId });
  return out;
}

async function main(): Promise<void> {
  const d = await startDaemon('v3');
  const c = await MisaoClient.connect(d.socket);
  result.daemonPid = d.pid;
  try {
    cases.a_normal_1000 = await runMarkers(d.socket, c, d.dir, 'N1', 1000, false);
    cases.slow_subscriber_gap = await slowSubscriber(d.socket, c, d.dir);
    cases.b_burst = await runMarkers(d.socket, c, d.dir, 'B1', BURST_ROUNDS, true);
  } finally {
    c.close();
    await d.stop();
  }
  result.finishedAt = new Date().toISOString();
  writeResult('v3', result);
}
main().catch((e) => {
  result.error = String(e?.stack ?? e);
  writeResult('v3', result);
  process.exit(1);
});
