import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, before, test } from 'node:test';
import { acquirePidFile } from '../src/pid-file.js';

let root: string;
before(() => {
  root = mkdtempSync(path.join(tmpdir(), 'misao-pid-'));
});
after(() => {
  rmSync(root, { recursive: true, force: true });
});

/** 終了済みのプロセスの pid。 */
function deadPid(): number {
  const r = spawnSync(process.execPath, ['-e', '']);
  return r.pid!;
}

test('acquirePidFile: 無ければ自分の pid で作る', () => {
  const p = path.join(root, 'new.pid');
  acquirePidFile(p, 4242);
  assert.equal(readFileSync(p, 'utf8'), '4242');
});

test('acquirePidFile: 生きているプロセスの pid ファイルがあれば例外で、書き換えない', () => {
  const p = path.join(root, 'alive.pid');
  writeFileSync(p, String(process.pid));
  assert.throws(() => acquirePidFile(p, 4242), /another daemon/);
  assert.equal(readFileSync(p, 'utf8'), String(process.pid));
});

test('acquirePidFile: 死んだプロセスや中身が pid でないファイルは置き換える', () => {
  for (const content of [String(deadPid()), 'garbage', '']) {
    const p = path.join(root, `stale-${content || 'empty'}.pid`);
    writeFileSync(p, content);
    acquirePidFile(p, 4242);
    assert.equal(readFileSync(p, 'utf8'), '4242');
  }
});
