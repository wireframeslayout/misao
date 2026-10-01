import assert from 'node:assert/strict';
import { test } from 'node:test';
import xterm from '@xterm/headless';
import type { Terminal } from '@xterm/headless';
import { flushTerminal, serializeSnapshot } from '../src/screen.js';

function newTerm(): Terminal {
  return new xterm.Terminal({ cols: 40, rows: 10, allowProposedApi: true });
}

async function feed(term: Terminal, data: string): Promise<void> {
  term.write(data);
  await flushTerminal(term);
}

test('serializeSnapshot: 端末モードを復元する', async () => {
  const src = newTerm();
  // DECCKM, DECKPAM, IRM, DECAWM off, focus, bracketed paste, mouse(vt200), DECOM
  await feed(src, 'hello\x1b[?1h\x1b=\x1b[4h\x1b[?7l\x1b[?1004h\x1b[?2004h\x1b[?1000h\x1b[?6h\x1b[3;5H');
  const dst = newTerm();
  await feed(dst, serializeSnapshot(src));
  const keys = [
    'applicationCursorKeysMode',
    'applicationKeypadMode',
    'insertMode',
    'wraparoundMode',
    'sendFocusMode',
    'bracketedPasteMode',
    'mouseTrackingMode',
    'originMode',
  ] as const;
  for (const k of keys) assert.equal(dst.modes[k], src.modes[k], k);
  assert.equal(dst.buffer.active.cursorX, src.buffer.active.cursorX);
  assert.equal(dst.buffer.active.cursorY, src.buffer.active.cursorY);
  src.dispose();
  dst.dispose();
});

test('serializeSnapshot: 既定のモードでは何も足さない (wraparound は on のまま)', async () => {
  const src = newTerm();
  await feed(src, 'plain');
  const out = serializeSnapshot(src);
  for (const seq of ['\x1b=', '\x1b[4h', '\x1b[?7l', '\x1b[?1004h', '\x1b[?6h']) assert.equal(out.includes(seq), false, seq);
  src.dispose();
});
