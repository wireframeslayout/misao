import { test } from 'node:test';
import assert from 'node:assert/strict';
import { commandNameFromArgv, parseForegroundPgid, sanitizeCommandName } from '../src/foreground.js';

test('parseForegroundPgid: tpgid を返す', () => {
  assert.equal(parseForegroundPgid('123 (bash) S 100 123 123 34816 4567 4194304 1 0'), 4567);
});

test('parseForegroundPgid: comm に ") (" を含んでも読める', () => {
  assert.equal(parseForegroundPgid('123 (a) (b) S 100 123 123 34816 789 4194304 1 0'), 789);
});

test('parseForegroundPgid: tpgid が -1 や読めない値なら undefined', () => {
  assert.equal(parseForegroundPgid('123 (bash) S 100 123 123 0 -1 4194304 1 0'), undefined);
  assert.equal(parseForegroundPgid('garbage'), undefined);
});

test('commandNameFromArgv: 通常のコマンドは argv[0] の basename', () => {
  assert.equal(commandNameFromArgv(['/usr/bin/sleep', '30']), 'sleep');
  assert.equal(commandNameFromArgv(['bash']), 'bash');
});

test('commandNameFromArgv: インタプリタはスクリプト名 (拡張子なし)', () => {
  assert.equal(commandNameFromArgv(['node', '/x/codex']), 'codex');
  assert.equal(commandNameFromArgv(['node', '--inspect', '/x/a.mjs']), 'a');
  assert.equal(commandNameFromArgv(['python3.12', '-u', 'x.py']), 'x');
});

test('commandNameFromArgv: スクリプト引数が無いインタプリタは argv[0] の basename', () => {
  assert.equal(commandNameFromArgv(['node']), 'node');
});

test('commandNameFromArgv: 空の argv は undefined', () => {
  assert.equal(commandNameFromArgv([]), undefined);
});

test('commandNameFromArgv: 制御文字を除く', () => {
  assert.equal(commandNameFromArgv(['\x1b]0;evil\x07vim']), ']0;evilvim');
  assert.equal(commandNameFromArgv(['node', '/x/\x1b[2Jcodex\n.js']), '[2Jcodex');
  assert.equal(commandNameFromArgv(['\x1b\x07']), undefined);
});

test('sanitizeCommandName: C1 制御文字を除く', () => {
  assert.equal(sanitizeCommandName('a\x9bb'), 'ab');
});

test('sanitizeCommandName: 64 コードポイントで切り、サロゲートペアを割らない', () => {
  assert.equal(sanitizeCommandName('x'.repeat(65)), 'x'.repeat(64));
  const emoji = '😀'.repeat(65);
  assert.equal(sanitizeCommandName(emoji), '😀'.repeat(64));
});
