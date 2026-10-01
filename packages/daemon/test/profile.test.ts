import { test } from 'node:test';
import assert from 'node:assert/strict';
import { selectProfile } from '../src/profile.js';
import type { AgentProfile } from '../src/profile.js';

const make = (name: string, bin: string): AgentProfile => ({
  name,
  matches: (cmd) => cmd[0] === bin,
  classify: () => null,
});

test('selectProfile は cmd に最初に matches したプロファイルを返す', () => {
  const a = make('a', 'agent');
  const b = make('b', 'agent');
  const c = make('c', 'other');
  assert.equal(selectProfile([a, b, c], ['agent', '--x']), a);
  assert.equal(selectProfile([a, b, c], ['other']), c);
});

test('selectProfile は matches するものが無ければ undefined を返す', () => {
  assert.equal(selectProfile([make('a', 'agent')], ['sh']), undefined);
  assert.equal(selectProfile([], ['sh']), undefined);
});
