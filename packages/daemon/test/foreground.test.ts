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

test('commandNameFromArgv: 値を取るオプションは値ごと飛ばす (--opt=value の形も)', () => {
  assert.equal(commandNameFromArgv(['node', '-r', 'dotenv/config', 'app.js']), 'app');
  assert.equal(commandNameFromArgv(['node', '--import', 'tsx', 'a.ts']), 'a');
  assert.equal(commandNameFromArgv(['node', '--import=tsx', '--conditions', 'dev', 'b.mjs']), 'b');
  assert.equal(commandNameFromArgv(['python3', '-W', 'ignore', '-X', 'dev', 'x.py']), 'x');
});

test('commandNameFromArgv: コードを直接渡すオプションはインタプリタ名', () => {
  assert.equal(commandNameFromArgv(['python3', '-c', 'import os; os.system("x")']), 'python3');
  assert.equal(commandNameFromArgv(['node', '-e', 'setTimeout(() => {}, 1)']), 'node');
  assert.equal(commandNameFromArgv(['node', '--eval=1']), 'node');
  assert.equal(commandNameFromArgv(['node', '-p', '1 + 1']), 'node');
  assert.equal(commandNameFromArgv(['node', '--print', '1']), 'node');
});

test('commandNameFromArgv: python -m はモジュール名をそのまま使う', () => {
  assert.equal(commandNameFromArgv(['python3', '-m', 'http.server', '8000']), 'http.server');
  assert.equal(commandNameFromArgv(['python3', '-m']), 'python3');
});

test('commandNameFromArgv: bun のサブコマンドは次の引数を使う', () => {
  assert.equal(commandNameFromArgv(['bun', 'run', 'dev']), 'dev');
  assert.equal(commandNameFromArgv(['bun', 'x', 'prettier', '.']), 'prettier');
  assert.equal(commandNameFromArgv(['bun', 'run', '--watch', 'src/index.ts']), 'index');
  assert.equal(commandNameFromArgv(['bun', 'server.ts']), 'server');
});

test('commandNameFromArgv: -- の次をスクリプトとみなす', () => {
  assert.equal(commandNameFromArgv(['node', '--', '-weird.js']), '-weird');
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

test('sanitizeCommandName: bidi・ゼロ幅・行区切りなどの書式文字を除く', () => {
  assert.equal(sanitizeCommandName('evil\u202egnp.exe'), 'evilgnp.exe');
  assert.equal(sanitizeCommandName('a\u200bb\u2028c\u2029d\ufeff'), 'abcd');
  assert.equal(sanitizeCommandName('\u202e\u200b'), undefined);
});

test('sanitizeCommandName: 64 コードポイントで切り、サロゲートペアを割らない', () => {
  assert.equal(sanitizeCommandName('x'.repeat(65)), 'x'.repeat(64));
  const emoji = '😀'.repeat(65);
  assert.equal(sanitizeCommandName(emoji), '😀'.repeat(64));
});
