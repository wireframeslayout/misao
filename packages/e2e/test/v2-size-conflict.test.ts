import * as fs from 'node:fs';
import * as path from 'node:path';
import { after, before, test } from 'node:test';
import type { MisaoClient } from '@misao/sdk';
import { openPane } from './helpers/agents.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { runSizeConflict } from './helpers/size-conflict.js';
import { waitFor } from './helpers/wait.js';

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(async () => {
  await daemon.stop();
});

test('v2: vim の pane サイズは最後に入力したクライアントのものになり、所有者が抜けると残った側に戻る', { timeout: 60_000 }, async () => {
  const file = path.join(daemon.dir, 'big.txt');
  fs.writeFileSync(file, Array.from({ length: 200 }, (_, i) => `line ${i} ` + 'x'.repeat(50)).join('\n') + '\n');
  const paneId = await openPane(client, ['vim', '-u', 'NONE', '-N', file]);
  await waitFor(async () => (await client.request('pane.screen', { paneId })).altScreen, 'vim to enter the alt screen');
  await runSizeConflict(daemon.socket, client, paneId, 'l');
  await client.request('pane.close', { paneId });
});
