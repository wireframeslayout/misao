import assert from 'node:assert/strict';
import { test } from 'node:test';
import { TTY_RESET, TtyGuard, terminalSize } from '../../src/attach/tty.js';
import { CliError } from '../../src/errors.js';
import { createTestIo } from '../helpers/io.js';

test('enter は raw mode に入り、restore は戻して TTY_RESET を 1 回だけ書く', () => {
  const io = createTestIo({ isTTY: true });
  const guard = new TtyGuard(io);
  guard.enter(() => undefined);
  assert.deepEqual(io.rawModes, [true]);
  guard.restore();
  guard.restore();
  assert.deepEqual(io.rawModes, [true, false]);
  assert.equal(io.out(), TTY_RESET);
});

test('SIGTERM / SIGHUP では端末を戻してから通知する', () => {
  for (const signal of ['SIGTERM', 'SIGHUP'] as const) {
    const io = createTestIo({ isTTY: true });
    const guard = new TtyGuard(io);
    const seen: boolean[] = [];
    guard.enter(() => seen.push(io.rawModes.at(-1) === false));
    io.emitSignal(signal);
    assert.deepEqual(seen, [true], signal);
    assert.equal(io.out(), TTY_RESET);
  }
});

test('捕捉されなかった例外でも端末を戻す', () => {
  const io = createTestIo({ isTTY: true });
  new TtyGuard(io).enter(() => undefined);
  io.emitUncaught(new Error('boom'));
  assert.deepEqual(io.rawModes, [true, false]);
  assert.equal(io.out(), TTY_RESET);
  assert.match(io.err(), /予期しないエラー: boom/);
});

test('restore 後はシグナルを扱わない / TTY でなければ usage エラー', () => {
  const io = createTestIo({ isTTY: true });
  const guard = new TtyGuard(io);
  let called = 0;
  guard.enter(() => called++);
  guard.restore();
  io.emitSignal('SIGTERM');
  assert.equal(called, 0);

  assert.throws(
    () => new TtyGuard(createTestIo()).enter(() => undefined),
    (e: unknown) => e instanceof CliError && e.kind === 'usage',
  );
});

test('TTY_RESET: alt screen / マウス / bracketed paste / カーソル表示を戻す', () => {
  for (const seq of ['?1049l', '?1000l', '?1002l', '?1003l', '?1006l', '?2004l', '?1l', '?25h']) {
    assert.ok(TTY_RESET.includes(seq), seq);
  }
});

test('terminalSize: 端末なら列と行、そうでなければ usage エラー', () => {
  assert.deepEqual(terminalSize(createTestIo({ isTTY: true, columns: 100, rows: 30 })), { cols: 100, rows: 30 });
  for (const io of [createTestIo({ columns: 100, rows: 30 }), createTestIo({ isTTY: true })]) {
    assert.throws(() => terminalSize(io), (e: unknown) => e instanceof CliError && e.kind === 'usage');
  }
});
