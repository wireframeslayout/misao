import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LOG_LEVELS, createLogger } from '../src/log.js';
import type { LogLevel } from '../src/log.js';

function emitAll(level: LogLevel): string[] {
  const out: string[] = [];
  const log = createLogger(level, (m) => out.push(m));
  for (const l of LOG_LEVELS) log[l](l);
  return out;
}

test('閾値以下の詳細度だけ sink に渡る', () => {
  assert.deepEqual(emitAll('error'), ['error']);
  assert.deepEqual(emitAll('warn'), ['error', 'warn']);
  assert.deepEqual(emitAll('info'), ['error', 'warn', 'info']);
  assert.deepEqual(emitAll('debug'), ['error', 'warn', 'info', 'debug']);
});
