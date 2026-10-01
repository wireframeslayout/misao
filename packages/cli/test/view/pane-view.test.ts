import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { PaneInfo } from '@misao/protocol';
import {
  displayName,
  foregroundCommand,
  formatWindowId,
  isRegistered,
  relativeTime,
  shortDisplayName,
  shortPaneIds,
  shortSuffix,
  sortPanes,
  stateLabel,
  taskLabel,
  tildify,
} from '../../src/view/pane-view.js';
import { makePane } from '../helpers/pane.js';

const HOME = '/home/test';


function panesOf(...paneIds: string[]): PaneInfo[] {
  return paneIds.map((paneId) => makePane({ paneId }));
}

test('sortPanes: blocked → working → idle → exited → stopped → unknown、同状態は新しい順', () => {
  const t = (s: number): string => new Date(Date.UTC(2026, 0, 1, 0, 0, s)).toISOString();
  const panes = [
    makePane({ agentState: 'idle', lastOutputAt: t(5) }),
    makePane({ agentState: 'unknown', lastOutputAt: t(50) }),
    makePane({ agentState: 'working', lastOutputAt: t(1) }),
    makePane({ agentState: 'working', lastOutputAt: t(9) }),
    makePane({ agentState: 'working', lastOutputAt: null }),
    makePane({ agentState: 'blocked', lastOutputAt: t(2) }),
    makePane({ agentState: 'exited', processState: 'exited', lastOutputAt: t(3) }),
    makePane({ agentState: 'idle', processState: 'stopped', lastOutputAt: t(40) }),
  ];
  const sorted = sortPanes(panes);
  assert.deepEqual(
    sorted.map((p) => [p.processState === 'stopped' ? 'stopped' : p.agentState, p.lastOutputAt]),
    [
      ['blocked', t(2)],
      ['working', t(9)],
      ['working', t(1)],
      ['working', null],
      ['idle', t(5)],
      ['exited', t(3)],
      ['stopped', t(40)],
      ['unknown', t(50)],
    ],
  );
  assert.notEqual(sorted, panes, '入力を変更しない');
  assert.equal(panes[0]!.agentState, 'idle');
});

test('stateLabel: 記号付き', () => {
  assert.equal(stateLabel(makePane({ agentState: 'blocked' })), '● blocked');
  assert.equal(stateLabel(makePane({ agentState: 'working' })), '◐ working');
  assert.equal(stateLabel(makePane({ agentState: 'idle' })), '○ idle');
  assert.equal(stateLabel(makePane({ agentState: 'exited', processState: 'exited' })), '✕ exited');
  assert.equal(stateLabel(makePane({ processState: 'stopped' })), '■ stopped');
});

test('displayName: 登録済みと未登録の 4 通り', () => {
  assert.equal(displayName(makePane({ labels: { windowId: '806', name: '表示名' } }), HOME), 'W-806 · 表示名');
  assert.equal(displayName(makePane({ labels: { windowId: '806' } }), HOME), 'W-806');
  assert.equal(displayName(makePane({ labels: { name: 'メモ' } }), HOME), '(未登録) メモ');
  assert.equal(
    displayName(makePane({ cmd: ['/usr/bin/claude'], cwd: '/home/test/workspace/azito', fgCommand: 'node' }), HOME),
    '(未登録) node · ~/workspace/azito',
  );
  assert.equal(displayName(makePane({ cmd: ['/usr/bin/claude'], cwd: '/srv/x' }), HOME), '(未登録) claude · /srv/x');
});

test('isRegistered / formatWindowId', () => {
  assert.equal(isRegistered(makePane({ labels: { windowId: '806' } })), true);
  assert.equal(isRegistered(makePane({ labels: { windowId: '' } })), false);
  assert.equal(isRegistered(makePane()), false);
  assert.equal(formatWindowId('806'), 'W-806');
  assert.equal(formatWindowId('w-806'), 'W-806');
});

test('tildify / foregroundCommand / taskLabel', () => {
  assert.equal(tildify('/home/test', HOME), '~');
  assert.equal(tildify('/home/test/a/b', HOME), '~/a/b');
  assert.equal(tildify('/home/tester/a', HOME), '/home/tester/a');
  assert.equal(foregroundCommand(makePane({ fgCommand: 'vim' })), 'vim');
  assert.equal(foregroundCommand(makePane({ fgCommand: '', cmd: ['/bin/zsh'] })), 'zsh');
  assert.equal(taskLabel(makePane({ labels: { task: '419' } })), '#419');
  assert.equal(taskLabel(makePane({ labels: { task: '#419' } })), '#419');
  assert.equal(taskLabel(makePane()), '—');
});

test('relativeTime: s / m / h と —', () => {
  const now = Date.parse('2026-01-01T12:00:00Z');
  const ago = (s: number): string => new Date(now - s * 1000).toISOString();
  assert.equal(relativeTime(null, now), '—');
  assert.equal(relativeTime(ago(12), now), '12s');
  assert.equal(relativeTime(ago(4 * 60 + 5), now), '4m');
  assert.equal(relativeTime(ago(2 * 3600 + 100), now), '2h');
  assert.equal(relativeTime(ago(-5), now), '0s', '未来の時刻は 0s');
});

test('shortPaneIds: 先頭 4 文字…末尾 2 文字。衝突したら末尾を伸ばす', () => {
  const a = 'p_01M3XXXXXXXXXXXXXXXXXXXX7Q';
  const b = 'p_01M3XXXXXXXXXXXXXXXXXXXXR8';
  const c = 'p_01M3XXXXXXXXXXXXXXXXXXXY7Q';
  const one = shortPaneIds(panesOf(a, b));
  assert.equal(one.get(a), 'p_01M3…7Q');
  assert.equal(one.get(b), 'p_01M3…R8');
  const three = shortPaneIds(panesOf(a, b, c));
  assert.equal(three.get(a), 'p_01M3…X7Q');
  assert.equal(three.get(c), 'p_01M3…Y7Q');
  assert.equal(three.get(b), 'p_01M3…R8');
  assert.equal(shortSuffix('p_01M3…R8'), 'R8');
});

test('shortPaneIds: 末尾は先頭 4 文字が違う pane や、他の ID の先頭とも衝突しない (対象指定で一意に引ける)', () => {
  const a = 'p_01M3XXXXXXXXXXXXXXXXXXXXR8';
  const b = 'p_01M4YYYYYYYYYYYYYYYYYYYYR8';
  const c = 'p_01M5ZZZZZZZZZZZZZZZZZZZZ01';
  const ids = shortPaneIds(panesOf(a, b, c));
  assert.equal(ids.get(a), 'p_01M3…XR8');
  assert.equal(ids.get(b), 'p_01M4…YR8');
  assert.equal(ids.get(c), 'p_01M5…Z01', '01 だと他の ID の先頭に当たる');
  const lower = shortPaneIds(panesOf('p_01M3XXXXXXXXXXXXXXXXXXXXab', 'p_01M4YYYYYYYYYYYYYYYYYYYYAB'));
  assert.equal(lower.get('p_01M3XXXXXXXXXXXXXXXXXXXXab'), 'p_01M3…Xab', '大文字小文字を区別せずに比べる');
});

test('shortPaneIds: 末尾が windowId と等しいケースでは、窓番号の段に当たらない長さまで伸ばす', () => {
  const a = 'p_01M3XXXXXXXXXXXXXXXXXXXXR8';
  const panes = [makePane({ paneId: a }), makePane({ paneId: 'p_01M4YYYYYYYYYYYYYYYYYYYYQQ', labels: { windowId: 'W-r8' } })];
  assert.equal(shortPaneIds(panes).get(a), 'p_01M3…XR8', 'R8 は W-r8 の窓番号と同じ');
});

test('shortPaneIds: 数字だけの末尾は窓番号として扱われるので、文字を含むまで伸ばす', () => {
  const a = 'p_01M3XXXXXXXXXXXXXXXXXXXK12';
  const b = 'p_01M3XXXXXXXXXXXXXXXXXX9812';
  const ids = shortPaneIds(panesOf(a, b));
  assert.equal(ids.get(a), 'p_01M3…K12');
  assert.equal(ids.get(b), 'p_01M3…X9812');
});

test('shortDisplayName: 登録済みは窓番号だけ、未登録は NAME 列と同じ', () => {
  assert.equal(shortDisplayName(makePane({ labels: { windowId: '806', name: '表示名' } }), HOME), 'W-806');
  assert.equal(shortDisplayName(makePane({ labels: { name: 'メモ' } }), HOME), '(未登録) メモ');
});
