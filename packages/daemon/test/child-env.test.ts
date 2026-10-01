import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildChildEnv } from '../src/child-env.js';

test('TMUX / TMUX_PANE / STY / ZELLIJ* を除去し、他の変数は残す', () => {
  const env = buildChildEnv(
    { PATH: '/bin', TMUX: 'x', TMUX_PANE: '%1', STY: 's', ZELLIJ: '0', ZELLIJ_SESSION_NAME: 'z', HOME: '/h' },
    'p_X',
    '/tmp/s.sock',
  );
  assert.equal(env.PATH, '/bin');
  assert.equal(env.HOME, '/h');
  for (const k of ['TMUX', 'TMUX_PANE', 'STY', 'ZELLIJ', 'ZELLIJ_SESSION_NAME']) assert.equal(k in env, false, k);
});

test('MISAO_SOCKET / MISAO_PANE_ID / TERM / COLORTERM を注入する', () => {
  const env = buildChildEnv({ TERM: 'dumb' }, 'p_X', '/tmp/s.sock');
  assert.equal(env.MISAO_SOCKET, '/tmp/s.sock');
  assert.equal(env.MISAO_PANE_ID, 'p_X');
  assert.equal(env.TERM, 'xterm-256color');
  assert.equal(env.COLORTERM, 'truecolor');
});

test('overrides が最後に上書きし、undefined の値は含めない', () => {
  const env = buildChildEnv({ A: undefined, B: '1' }, 'p_X', '/s', { TERM: 'vt100', C: '3' });
  assert.equal(env.TERM, 'vt100');
  assert.equal(env.C, '3');
  assert.equal('A' in env, false);
});
