import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MisaoClient } from '../src/client.js';
import { MisaoConnectionError } from '../src/errors.js';

// since と epoch は組で渡す。片方だけは型エラー (tsc -p tsconfig.test.json が検証する)。
test('since and epoch must be passed together', async () => {
  const client = new MisaoClient({ socketPath: '/nonexistent.sock' });
  // @ts-expect-error since だけでは epoch をまたいだ位置を検出できない
  await assert.rejects(client.subscribeEvents(() => undefined, { since: 3 }), MisaoConnectionError);
  // @ts-expect-error epoch だけの指定は意味を持たない
  await assert.rejects(client.subscribeEvents(() => undefined, { epoch: 'x' }), MisaoConnectionError);
  await assert.rejects(client.subscribeEvents(() => undefined, { since: 3, epoch: 'x' }), MisaoConnectionError);
});
