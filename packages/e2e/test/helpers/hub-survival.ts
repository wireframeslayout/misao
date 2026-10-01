import assert from 'node:assert/strict';
import type { MisaoClient } from '@misao/sdk';
import { MARKER_RE, paneInfo, readAgentSummary, screenText } from './agents.js';
import { readHubLog } from './hub-process.js';
import { waitFor } from './wait.js';

export interface PaneSnapshot {
  pid: number | null;
  processState: string;
  screen: string;
}

export async function snapshotPane(client: MisaoClient, paneId: string): Promise<PaneSnapshot> {
  const info = await paneInfo(client, paneId);
  return { pid: info.pid, processState: info.processState, screen: await screenText(client, paneId) };
}

/** hub を落として戻した前後で、pane のプロセスと画面が影響を受けていないことを確かめる。 */
export function assertPaneSurvived(label: string, name: string, before: PaneSnapshot, after: PaneSnapshot, isScreenStable: boolean): void {
  assert.equal(after.pid, before.pid, `${label}: ${name} の PID が同じ`);
  assert.equal(after.processState, 'running', `${label}: ${name} が動いている`);
  if (isScreenStable) assert.equal(after.screen, before.screen, `${label}: ${name} の画面が変わらない`);
  else assert.notEqual(after.screen.trim(), '', `${label}: ${name} の画面が空でない`);
}

/** seq 列の重複数、先頭の seq、欠け (連続でない箇所の合計)。 */
function analyzeSeqs(seqs: number[]): { duplicates: number; first: number | undefined; holes: number } {
  const unique = [...new Set(seqs)].sort((a, b) => a - b);
  let holes = 0;
  for (let i = 1; i < unique.length; i++) holes += unique[i]! - unique[i - 1]! - 1;
  return { duplicates: seqs.length - unique.length, first: unique[0], holes };
}

/**
 * agent の完了後、hub のログが agent の全マーカーと連続した seq を含むことを確かめる。
 * ログ追記 → state 保存の順なので、hub を kill -9 した回ごとに高々 1 件 (行かイベントのどちらか) が重複しうる (欠けはしない)。
 * 再起動 (SIGTERM) は処理の区切りで終わるので重複しない。hub は since 0 から購読するので、seq は 1 から始まる。
 */
export async function assertHubLogComplete(
  client: MisaoClient,
  { logFile, agentPaneId, agentId, summaryFile, killCount }: { logFile: string; agentPaneId: string; agentId: string; summaryFile: string; killCount: number },
): Promise<void> {
  const { head } = await client.request('pane.subscribe_lines', { paneId: agentPaneId });
  const agentLines = () => readHubLog(logFile).filter((e) => e.kind === 'line' && e.paneId === agentPaneId);
  await waitFor(() => agentLines().at(-1)?.seq === head, 'the hub to catch up with the agent lines', { timeoutMs: 20_000 });

  const expected = readAgentSummary(summaryFile).nonces.map((nonce) => `AZITO_DONE_${agentId}_${nonce}`);
  const found = new Set(agentLines().flatMap((e) => [...e.text!.matchAll(MARKER_RE)].map((m) => m[0])));
  assert.deepEqual(expected.filter((marker) => !found.has(marker)), [], '取り逃したマーカー');
  assert.deepEqual([...found].filter((marker) => !expected.includes(marker)), [], '誤検出');
  const lines = analyzeSeqs(agentLines().map((e) => e.seq!));
  const events = analyzeSeqs(readHubLog(logFile).filter((e) => e.kind === 'event').map((e) => e.seq!));
  assert.equal(lines.first, 1, '行 seq が 1 から始まる');
  assert.equal(events.first, 1, 'イベント seq が 1 から始まる');
  assert.equal(lines.holes, 0, '行 seq に欠けがない');
  assert.equal(events.holes, 0, 'イベント seq に欠けがない');
  assert.ok(lines.duplicates + events.duplicates <= killCount, `重複は kill -9 の回数以内: ${JSON.stringify({ lines, events, killCount })}`);
}
