import assert from 'node:assert/strict';
import { test } from 'node:test';
import { CliError } from '../src/errors.js';
import { resolveTarget } from '../src/target.js';
import { makePane } from './helpers/pane.js';

const HOME = '/home/test';

function expectError(fn: () => unknown, kind: CliError['kind']): CliError {
  try {
    fn();
  } catch (e) {
    assert.ok(e instanceof CliError, String(e));
    assert.equal(e.kind, kind);
    return e;
  }
  assert.fail('エラーにならなかった');
}

const a = makePane({
  paneId: 'p_01M3XXXXXXXXXXXXXXXXXXXX7Q',
  labels: { windowId: '806', name: 'misao 計画', task: '419', agent: 'claude-1' },
});
const b = makePane({ paneId: 'p_01M3XXXXXXXXXXXXXXXXXXXXR8', labels: { windowId: '807', name: 'misao 実装', task: '#437' } });
const c = makePane({ paneId: 'p_07ZZYYYYYYYYYYYYYYYYYYYYK1', window: { id: 'w_01M3AAAAAAAAAAAAAAAAAAAAAA', name: 'scratch' } });
const panes = [a, b, c];

test('ID 完全一致が最優先', () => {
  assert.equal(resolveTarget(a.paneId, panes, HOME), a);
});

test('ID の前方・後方一致 (p_ の有無・大文字小文字を問わない)', () => {
  assert.equal(resolveTarget('07zzy', panes, HOME), c);
  assert.equal(resolveTarget('p_07zz', panes, HOME), c);
  assert.equal(resolveTarget('r8', panes, HOME), b);
  assert.equal(resolveTarget('K1', panes, HOME), c);
});

test('task:N / agent:ID (task は # を剥がして比べる)', () => {
  assert.equal(resolveTarget('task:419', panes, HOME), a);
  assert.equal(resolveTarget('task:437', panes, HOME), b);
  assert.equal(resolveTarget('task:#437', panes, HOME), b);
  assert.equal(resolveTarget('agent:claude-1', panes, HOME), a);
});

test('窓番号は 806 でも W-806 でも当たる', () => {
  assert.equal(resolveTarget('806', panes, HOME), a);
  assert.equal(resolveTarget('W-807', panes, HOME), b);
  assert.equal(resolveTarget('w-807', panes, HOME), b);
});

test('名前の部分一致 (name ラベルと daemon の窓名)', () => {
  assert.equal(resolveTarget('計画', panes, HOME), a);
  assert.equal(resolveTarget('SCRATCH', panes, HOME), c);
});

test('段階の早いものが勝つ: 窓番号に当たれば ID の一致があっても窓番号で決める', () => {
  const d = makePane({ paneId: 'p_01M3XXXXXXXXXXXXXXXXXXXX88', labels: {} });
  const e = makePane({ paneId: 'p_01M3XXXXXXXXXXXXXXXXXXXX99', labels: { windowId: '88' } });
  assert.equal(resolveTarget('88', [d, e], HOME), e);
});

test('0 件は not_found、複数は ambiguous で候補を返す', () => {
  expectError(() => resolveTarget('nothing-here', panes, HOME), 'not_found');
  const err = expectError(() => resolveTarget('misao', panes, HOME), 'ambiguous');
  assert.deepEqual(
    err.candidates?.map((x) => x.paneId),
    [a.paneId, b.paneId],
  );
  assert.equal(err.candidates?.[0]?.name, 'W-806 · misao 計画');
  expectError(() => resolveTarget('', panes, HOME), 'usage');
});

test('窓番号の形のクエリは窓番号の段だけで決め、閉じた窓番号で別のペインに落ちない', () => {
  const byId = makePane({ paneId: 'p_01M3XXXXXXXXXXXXXXXXXXX806', labels: {} });
  const byName = makePane({ paneId: 'p_01M3XXXXXXXXXXXXXXXXXXXXKK', labels: { name: 'issue-806-fix' } });
  const err = expectError(() => resolveTarget('806', [byId, byName], HOME), 'not_found');
  assert.match(err.message, /^W-806 は見つかりません。misao ls で確認してください/);
  assert.match(err.message, /p_ を付けて/);
  expectError(() => resolveTarget('W-806', [byId, byName], HOME), 'not_found');
  assert.equal(resolveTarget('p_806', [byId, byName], HOME), byId, '数字だけの ID は p_ を付ければ ID として引ける');
});

test('ID の部分一致は 2 文字以上。1 文字では確定しない', () => {
  const only = makePane({ paneId: 'p_01M3XXXXXXXXXXXXXXXXXXXXQZ', labels: {} });
  expectError(() => resolveTarget('Z', [only], HOME), 'not_found');
  expectError(() => resolveTarget('p_0', [only], HOME), 'not_found');
  assert.equal(resolveTarget('QZ', [only], HOME), only);
});
