import assert from 'node:assert/strict';
import { test } from 'node:test';
import { attachablePanes, resumedNotice, runPicker } from '../../src/attach/picker.js';
import { createTestIo } from '../helpers/io.js';
import { makePane } from '../helpers/pane.js';

const HOME = '/home/test';
const blocked = makePane({ agentState: 'blocked', labels: { windowId: '806', name: '計画', task: '419' }, fgCommand: 'claude' });
const idle = makePane({ agentState: 'idle', labels: { name: 'メモ' } });
const exited = makePane({ agentState: 'exited', processState: 'exited' });
const stopped = makePane({ processState: 'stopped' });
const options = { header: [], canCreate: true };

test('attachablePanes: exited / stopped を除き、ls と同じ並び', () => {
  assert.deepEqual(attachablePanes([idle, exited, stopped, blocked]).map((p) => p.paneId), [blocked.paneId, idle.paneId]);
});

test('resumedNotice: blocked だったものが今 working のときだけ知らせる', () => {
  const working = { ...blocked, agentState: 'working' as const };
  assert.match(resumedNotice(blocked, working, HOME) ?? '', /W-806 はいま再開しました/);
  assert.equal(resumedNotice(blocked, blocked, HOME), undefined);
  assert.equal(resumedNotice(idle, { ...idle, agentState: 'working' }, HOME), undefined);
});

test('一覧に番号を付け、案内を出し、番号で選べる', async () => {
  const io = createTestIo();
  const chosen = runPicker(io, [idle, exited, blocked], { header: ['[misao] W-806 から抜けました（入力 3 回を記録。内容は保存していません）'], canCreate: true });
  io.stdin.write('2\n');
  const choice = await chosen;
  assert.deepEqual(choice.kind === 'pane' && choice.pane.paneId, idle.paneId);
  const lines = io.out().trimEnd().split('\n');
  assert.equal(lines[0], '[misao] W-806 から抜けました（入力 3 回を記録。内容は保存していません）');
  assert.match(lines[1]!, /^ {2}# +STATE +NAME +TASK +AGENT +LAST$/);
  assert.match(lines[2]!, /^ {2}1 +● blocked +W-806 · 計画 +#419 +claude /);
  assert.match(lines[3]!, /^ {2}2 +○ idle +\(未登録\) メモ /);
  assert.equal(lines[4], '番号で入る · n で新しいペイン · q で終了');
  assert.equal(lines.length, 5, 'exited は載せない');
});

test('n は新しいペイン、q と入力の終端は終了', async () => {
  const a = createTestIo();
  a.stdin.write('N\n');
  assert.deepEqual(await runPicker(a, [idle], options), { kind: 'new' });
  const b = createTestIo();
  b.stdin.write('q\n');
  assert.deepEqual(await runPicker(b, [idle], options), { kind: 'quit' });
  const c = createTestIo();
  c.stdin.end();
  assert.deepEqual(await runPicker(c, [idle], options), { kind: 'quit' });
});

test('範囲外・不正な入力は案内して聞き直す。canCreate=false なら n も不正', async () => {
  const io = createTestIo();
  const chosen = runPicker(io, [idle], { header: [], canCreate: false });
  io.stdin.write('9\n');
  await new Promise((r) => setTimeout(r, 50));
  io.stdin.write('n\n');
  await new Promise((r) => setTimeout(r, 50));
  io.stdin.write('1\n');
  assert.equal((await chosen).kind, 'pane');
  assert.equal(io.err().match(/のいずれかを入力してください/g)?.length, 2);
  assert.doesNotMatch(io.out(), /n で新しいペイン/);
});

test('入れるペインが無くても n / q は受け付ける', async () => {
  const io = createTestIo();
  io.stdin.write('q\n');
  assert.deepEqual(await runPicker(io, [exited], options), { kind: 'quit' });
  assert.match(io.out(), /入れるペインがありません/);
});

test('入力待ちで SIGTERM / SIGHUP / SIGINT / SIGQUIT を受けたら signal を返し、ハンドラを外す', async () => {
  for (const signal of ['SIGTERM', 'SIGHUP', 'SIGINT', 'SIGQUIT'] as const) {
    const io = createTestIo();
    const chosen = runPicker(io, [idle], options);
    await new Promise((r) => setTimeout(r, 10));
    io.emitSignal(signal);
    assert.deepEqual(await chosen, { kind: 'signal' }, signal);
    assert.equal(io.stdin.isPaused(), true, `${signal}: 入力の読み取りを止めている`);
  }
  const io = createTestIo();
  io.stdin.write('q\n');
  assert.deepEqual(await runPicker(io, [idle], options), { kind: 'quit' });
  assert.doesNotThrow(() => io.emitSignal('SIGTERM'), '終わった後はハンドラが残っていない');
});
