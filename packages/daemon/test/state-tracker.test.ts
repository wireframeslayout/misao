import { test } from 'node:test';
import type { TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { StateTracker } from '../src/state-tracker.js';
import type { AgentState } from '../src/state-tracker.js';
import type { AgentProfile, ProfileScreen, ProfileVerdict } from '../src/profile.js';

const SCREEN: ProfileScreen = { rows: ['hello'], title: '', altScreen: false };

interface Change {
  state: AgentState;
  decidedBy: string;
  prev: AgentState;
}

/** 時刻を手で進める。タイマー (1 秒ごとの tick) は実時間なので、テストでは止めて使う。 */
function setup(profile?: AgentProfile): { tracker: StateTracker; changes: Change[]; clock: { now: number }; logs: string[] } {
  const changes: Change[] = [];
  const logs: string[] = [];
  const clock = { now: 0 };
  const tracker = new StateTracker({
    profile,
    readScreen: () => SCREEN,
    onChange: (state, decidedBy, prev) => changes.push({ state, decidedBy, prev }),
    now: () => clock.now,
    log: { warn: (m) => logs.push(m) },
  });
  return { tracker, changes, clock, logs };
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

/** プロファイルの debounce は setTimeout なので、mock.timers で時間を手で進める。 */
function mockTimers(t: TestContext): void {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
}

test('プロファイルは title / bytes より優先され、blocked を出せる。null なら次の段へ落ちる', (t) => {
  mockTimers(t);
  const verdicts: { current: ProfileVerdict } = { current: 'blocked' };
  const { tracker, changes, clock } = setup(fakeProfile(verdicts));
  tracker.notifyScreenUpdated();
  t.mock.timers.tick(120);
  assert.deepEqual(changes, [{ state: 'blocked', decidedBy: 'fake', prev: 'unknown' }]);
  clock.now = 10;
  tracker.setTitle('◐ task'); // title は working だが、プロファイルの方が優先
  assert.equal(tracker.snapshot().agentState, 'blocked');
  verdicts.current = null;
  tracker.notifyScreenUpdated();
  t.mock.timers.tick(120);
  assert.deepEqual(changes.at(-1), { state: 'working', decidedBy: 'title', prev: 'blocked' });
  tracker.stop();
});

test('プロファイルの判定は画面更新の通知でだけ予約される (出力の受信では読まない)', (t) => {
  mockTimers(t);
  let calls = 0;
  const profile: AgentProfile = { name: 'fake', matches: () => true, classify: () => (calls++, 'working') };
  const { tracker } = setup(profile);
  tracker.recordOutput(10);
  t.mock.timers.tick(500);
  assert.equal(calls, 0, '解析前の画面を読まない');
  tracker.notifyScreenUpdated();
  t.mock.timers.tick(120);
  assert.equal(calls, 1);
  tracker.stop();
});

test('プロファイルの判定は debounce され、更新が続いても最初の更新から 300ms で 1 度は判定する', (t) => {
  mockTimers(t);
  let calls = 0;
  const profile: AgentProfile = { name: 'fake', matches: () => true, classify: () => (calls++, 'working') };
  const clock = { now: 0 };
  const tracker = new StateTracker({
    profile,
    readScreen: () => SCREEN,
    onChange: () => undefined,
    now: () => clock.now,
    log: { warn: () => undefined },
  });
  const updateAfter = (ms: number): void => {
    t.mock.timers.tick(ms);
    clock.now += ms;
    tracker.notifyScreenUpdated();
  };
  updateAfter(0);
  for (let i = 0; i < 4; i++) updateAfter(60); // 240ms: 120ms の静けさが来ない
  assert.equal(calls, 0);
  t.mock.timers.tick(60); // 最初の更新から 300ms
  assert.equal(calls, 1);
  tracker.stop();
});

test('stop / markExited の後は、遅れて届く画面更新の通知で判定しない', (t) => {
  mockTimers(t);
  let calls = 0;
  const profile: AgentProfile = { name: 'fake', matches: () => true, classify: () => (calls++, 'working') };
  const { tracker } = setup(profile);
  tracker.stop();
  tracker.notifyScreenUpdated();
  t.mock.timers.tick(1000);
  assert.equal(calls, 0);
});

test('markExited は exit で終端し、以後の出力・タイトル・tick では変わらない', (t) => {
  mockTimers(t);
  const verdicts: { current: ProfileVerdict } = { current: 'working' };
  const { tracker, changes, clock } = setup(fakeProfile(verdicts));
  tracker.notifyScreenUpdated(); // プロファイル判定の予約中に終了しても、後から上書きされない
  tracker.markExited();
  assert.deepEqual(changes, [{ state: 'exited', decidedBy: 'exit', prev: 'unknown' }]);
  tracker.recordOutput(500);
  tracker.notifyScreenUpdated();
  tracker.setTitle('◐ task');
  tickAt(tracker, clock, 10_000);
  t.mock.timers.tick(1000);
  assert.deepEqual(tracker.snapshot(), { agentState: 'exited', decidedBy: 'exit' });
  assert.equal(changes.length, 1);
});

test('classify が throw しても tracker は落ちず、ログを残して title / bytes 段に落ちる', (t) => {
  mockTimers(t);
  const profile: AgentProfile = {
    name: 'broken',
    matches: () => true,
    classify: () => {
      throw new Error('boom');
    },
  };
  const { tracker, changes, clock, logs } = setup(profile);
  clock.now = 10;
  tracker.setTitle('◐ task');
  tracker.notifyScreenUpdated();
  assert.doesNotThrow(() => t.mock.timers.tick(120));
  assert.deepEqual(changes, [{ state: 'working', decidedBy: 'title', prev: 'unknown' }]);
  assert.match(logs.join('\n'), /profile broken failed \(1\/3\): boom/);
  tracker.stop();
});

test('classify が 3 回続けて失敗したら、その pane ではプロファイルを無効にして以後は予約しない', (t) => {
  mockTimers(t);
  let calls = 0;
  const profile: AgentProfile = {
    name: 'broken',
    matches: () => true,
    classify: () => {
      calls++;
      throw new Error('boom');
    },
  };
  const { tracker, logs } = setup(profile);
  for (let i = 0; i < 5; i++) {
    tracker.notifyScreenUpdated();
    t.mock.timers.tick(120);
  }
  assert.equal(calls, 3);
  assert.match(logs.join('\n'), /profile broken disabled after 3 consecutive failures/);
  tracker.stop();
});

test('classify の失敗は、成功を挟めば連続とみなさない', (t) => {
  mockTimers(t);
  const results: Array<'throw' | ProfileVerdict> = ['throw', 'throw', 'working', 'throw', 'throw', 'idle'];
  let calls = 0;
  const profile: AgentProfile = {
    name: 'flaky',
    matches: () => true,
    classify: () => {
      const r = results[calls++];
      if (r === 'throw') throw new Error('boom');
      return r ?? null;
    },
  };
  const { tracker, changes } = setup(profile);
  for (let i = 0; i < results.length; i++) {
    tracker.notifyScreenUpdated();
    t.mock.timers.tick(120);
  }
  assert.equal(calls, results.length);
  assert.deepEqual(changes.at(-1), { state: 'idle', decidedBy: 'flaky', prev: 'working' });
  tracker.stop();
});
