import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseCommandArgs, splitCommand } from '../src/args.js';
import { CliError } from '../src/errors.js';

const SPECS = {
  state: { type: 'string' },
  label: { type: 'string', multiple: true },
  force: { type: 'boolean' },
  since: { type: 'string' },
} as const;

test('splitCommand: コマンド名の前後どちらのグローバルオプションも読む', () => {
  const a = splitCommand(['--json', 'ls', '--state', 'blocked']);
  assert.equal(a.command, 'ls');
  assert.deepEqual(a.args, ['--json', '--state', 'blocked']);
  assert.equal(a.globals.flag('json'), true);

  const b = splitCommand(['--config', '/x.json', 'status']);
  assert.equal(b.command, 'status');
  assert.equal(b.globals.string('config'), '/x.json');
  assert.deepEqual(b.args, ['--config', '/x.json']);
});

test('splitCommand: コマンドが無ければ undefined、--version / --help は読める', () => {
  assert.equal(splitCommand([]).command, undefined);
  assert.equal(splitCommand(['--version']).globals.flag('version'), true);
  assert.equal(splitCommand(['--help']).globals.flag('help'), true);
});

test('splitCommand: -- 以降の --version はコマンドの引数であり、グローバルとして読まない', () => {
  const s = splitCommand(['new', '--', 'claude', '--version']);
  assert.equal(s.command, 'new');
  assert.equal(s.globals.flag('version'), false);
});

test('parseCommandArgs: 位置引数・値付き・複数指定・真偽', () => {
  const p = parseCommandArgs(['abc', '--label', 'a=1', '--label', 'b=2', '--force', '--state', 'idle'], SPECS);
  assert.deepEqual(p.positionals, ['abc']);
  assert.deepEqual(p.strings('label'), ['a=1', 'b=2']);
  assert.equal(p.flag('force'), true);
  assert.equal(p.string('state'), 'idle');
  assert.equal(p.string('since'), undefined);
  assert.deepEqual(p.strings('nothing'), []);
});

test('parseCommandArgs: -- 以降は rest に入り、位置引数にも値にも混ざらない', () => {
  const p = parseCommandArgs(['--label', 'x=1', '--', 'claude', '--force', 'a b'], SPECS);
  assert.deepEqual(p.rest, ['claude', '--force', 'a b']);
  assert.deepEqual(p.positionals, []);
  assert.equal(p.flag('force'), false);
});

test('parseCommandArgs: 未知のオプションと値の欠落は usage エラー', () => {
  for (const argv of [['--nope'], ['--state']]) {
    assert.throws(
      () => parseCommandArgs(argv, SPECS),
      (e: unknown) => e instanceof CliError && e.kind === 'usage' && e.exitCode === 2,
    );
  }
});

test('int: 整数と下限を検証する', () => {
  assert.equal(parseCommandArgs(['--since', '0'], SPECS).int('since', 0), 0);
  assert.equal(parseCommandArgs([], SPECS).int('since', 0), undefined);
  for (const bad of ['-1', '1.5', 'abc', '']) {
    assert.throws(
      () => parseCommandArgs(['--since', bad], SPECS).int('since', 0),
      (e: unknown) => e instanceof CliError && e.kind === 'usage',
    );
  }
  assert.throws(() => parseCommandArgs(['--since', '0'], SPECS).int('since', 1), CliError);
});
