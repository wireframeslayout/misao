import { test } from 'node:test';
import assert from 'node:assert/strict';
import { StateTracker } from '../src/state-tracker.js';
import type { AgentState } from '../src/state-tracker.js';
import type { AgentProfile, ProfileScreen, ProfileVerdict } from '../src/profile.js';

const SCREEN: ProfileScreen = { rows: ['hello'], title: '', altScreen: false };
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

interface Change {
  state: AgentState;
  decidedBy: string;
  prev: AgentState;
}

/** 時刻を手で進める。タイマー (1 秒ごとの tick) は実時間なので、テストでは止めて使う。 */
function setup(profile?: AgentProfile): { tracker: StateTracker; changes: Change[]; clock: { now: number } } {
  const changes: Change[] = [];
  const clock = { now: 0 };
  const tracker = new StateTracker({
    profile,
    readScreen: () => SCREEN,
    onChange: (state, decidedBy, prev) => changes.push({ state, decidedBy, prev }),
    now: () => clock.now,
  });
  return { tracker, changes, clock };
}

/** private な tick を、時刻を進めて呼ぶ。 */
function tickAt(t: StateTracker, clock: { now: number }, now: number): void {
  clock.now = now;
  (t as unknown as { tick(): void }).tick();
}

function fakeProfile(verdicts: { current: ProfileVerdict }): AgentProfile {
  return { name: 'fake', matches: () => true, classify: () => verdicts.current };
}

test('初期状態は unknown / none', () => {
  const { tracker } = setup();
  assert.deepEqual(tracker.snapshot(), { agentState: 'unknown', decidedBy: 'none' });
  tracker.stop();
});

test('bytes 段: 静かなまま 5 秒で idle、出力が続けば working (prev が付く)', () => {
  const { tracker, changes, clock } = setup();
  tickAt(tracker, clock, 5000);
  assert.deepEqual(changes, [{ state: 'idle', decidedBy: 'bytes', prev: 'unknown' }]);
  for (const t of [5100, 6100]) {
    clock.now = t;
    tracker.recordOutput(300);
    tickAt(tracker, clock, t + 900);
  }
  assert.deepEqual(changes.at(-1), { state: 'working', decidedBy: 'bytes', prev: 'idle' });
  tracker.stop();
});

test('title 段は bytes 段より優先される', () => {
  const { tracker, changes, clock } = setup();
  tickAt(tracker, clock, 5000); // bytes: idle
  clock.now = 5100;
  tracker.setTitle('◐ task');
  assert.deepEqual(changes.at(-1), { state: 'working', decidedBy: 'title', prev: 'idle' });
  tickAt(tracker, clock, 5200); // bytes は idle のままだが title が working を保つ
  assert.equal(tracker.snapshot().agentState, 'working');
  tracker.stop();
});

test('スピナーが 3 秒止まると title 段は意見を手放し、bytes 段に戻る', () => {
  const { tracker, changes, clock } = setup();
  clock.now = 100;
  tracker.setTitle('◐ task');
  tickAt(tracker, clock, 5000); // 4900ms 更新なし + bytes は idle
  assert.deepEqual(changes.at(-1), { state: 'idle', decidedBy: 'bytes', prev: 'working' });
  tracker.stop();
});

test('プロファイルは title / bytes より優先され、blocked を出せる。null なら次の段へ落ちる', async () => {
  const verdicts: { current: ProfileVerdict } = { current: 'blocked' };
  const { tracker, changes, clock } = setup(fakeProfile(verdicts));
  tracker.recordOutput(10);
  await sleep(200);
  assert.deepEqual(changes, [{ state: 'blocked', decidedBy: 'fake', prev: 'unknown' }]);
  clock.now = 10;
  tracker.setTitle('◐ task'); // title は working だが、プロファイルの方が優先
  assert.equal(tracker.snapshot().agentState, 'blocked');
  verdicts.current = null;
  tracker.recordOutput(10);
  await sleep(200);
  assert.deepEqual(changes.at(-1), { state: 'working', decidedBy: 'title', prev: 'blocked' });
  tracker.stop();
});

test('プロファイルの判定は debounce され、出力が続いても 300ms 以内に 1 度は判定する', async () => {
  let calls = 0;
  const profile: AgentProfile = { name: 'fake', matches: () => true, classify: () => (calls++, 'working') };
  const changes: Change[] = [];
  const tracker = new StateTracker({
    profile,
    readScreen: () => SCREEN,
    onChange: (state, decidedBy, prev) => changes.push({ state, decidedBy, prev }),
    now: Date.now,
  });
  for (let i = 0; i < 4; i++) {
    tracker.recordOutput(1);
    await sleep(60);
  }
  assert.equal(calls, 0, '出力が 60ms 間隔で続く間は、120ms の静けさが来ないので判定しない');
  for (let i = 0; i < 3; i++) {
    tracker.recordOutput(1);
    await sleep(60);
  }
  assert.ok(calls >= 1, '最初の出力から 300ms で、出力が続いていても判定する');
  assert.equal(changes[0]?.state, 'working');
  tracker.stop();
});

test('markExited は exit で終端し、以後の出力・タイトル・tick では変わらない', async () => {
  const verdicts: { current: ProfileVerdict } = { current: 'working' };
  const { tracker, changes, clock } = setup(fakeProfile(verdicts));
  tracker.recordOutput(10); // プロファイル判定の予約中に終了しても、後から上書きされない
  tracker.markExited();
  assert.deepEqual(changes, [{ state: 'exited', decidedBy: 'exit', prev: 'unknown' }]);
  tracker.recordOutput(500);
  tracker.setTitle('◐ task');
  tickAt(tracker, clock, 10_000);
  await sleep(200);
  assert.deepEqual(tracker.snapshot(), { agentState: 'exited', decidedBy: 'exit' });
  assert.equal(changes.length, 1);
});
