import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { after, before, describe, test } from 'node:test';
import type { MisaoClient } from '@misao/sdk';
import { openPane, screenText, typeKeys } from './helpers/agents.js';
import { ClientView } from './helpers/client-view.js';
import type { Replay } from './helpers/client-view.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { sleep, waitFor, waitQuiet } from './helpers/wait.js';

const REPLAYS: Replay[] = ['raw', 'snapshot'];
const RAW_RING_BYTES = 1024 * 1024;
const BULK_COMMAND = `seq 1 40000 | sed 's/$/ padding-text-to-lengthen-the-line-0123456789abcdef/'\r`;

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(async () => {
  await daemon.stop();
});

/** 出力バイト数を数えるだけの観測者を attach して返す。 */
async function observe(paneId: string, clientId: string): Promise<ClientView> {
  return ClientView.attach(daemon.socket, paneId, clientId, 'none', { cols: 100, rows: 30 });
}

/** 途中参加した視点が、replay の種類によらずデーモンの画面に追いつくことを確かめる。 */
async function assertJoinsConverge(paneId: string, tag: string): Promise<void> {
  await waitQuiet(client, paneId, { quietMs: 600 });
  for (const replay of REPLAYS) {
    const view = await ClientView.attach(daemon.socket, paneId, `join-${tag}-${replay}`, replay, { cols: 100, rows: 30 });
    try {
      await view.waitForMatch(client);
    } finally {
      view.close();
    }
  }
}

describe('v2: 途中参加の画面再構成', () => {
  test('bash の大量出力 (raw リング超過) の後に参加しても画面が一致する', { timeout: 120_000 }, async () => {
    const paneId = await openPane(client, ['bash', '--norc']);
    const monitor = await observe(paneId, 'monitor-a');
    await typeKeys(client, paneId, BULK_COMMAND);
    await waitFor(() => monitor.bytes > RAW_RING_BYTES + 256 * 1024, 'bulk output beyond the raw ring', { timeoutMs: 60_000 });
    await assertJoinsConverge(paneId, 'a');
    monitor.close();
    await client.request('pane.close', { paneId });
  });

  test('vim (alt screen) 編集中に参加しても画面が一致する。リング超過後も一致する', { timeout: 180_000 }, async () => {
    const big = path.join(daemon.dir, 'big.txt');
    fs.writeFileSync(big, Array.from({ length: 5000 }, (_, i) => `line ${i} ` + 'x'.repeat(50)).join('\n') + '\n');
    const paneId = await openPane(client, ['bash', '--norc']);
    const monitor = await observe(paneId, 'monitor-b');
    await typeKeys(client, paneId, BULK_COMMAND);
    await waitFor(() => monitor.bytes > RAW_RING_BYTES + 256 * 1024, 'bulk output beyond the raw ring', { timeoutMs: 60_000 });
    await waitQuiet(client, paneId, { quietMs: 1000 });
    const beforeVim = monitor.bytes;
    await typeKeys(client, paneId, `vim -u NONE -N ${big}\r`);
    await waitFor(async () => (await client.request('pane.screen', { paneId })).altScreen, 'vim to enter the alt screen');
    await typeKeys(client, paneId, '10Gihello edit\x1b:set number\r5G');
    await waitFor(async () => (await screenText(client, paneId)).includes('hello edit'), 'the edit to appear');
    await assertJoinsConverge(paneId, 'b');

    // vim の再描画だけで raw リングを超過させてから参加する (alt screen 内での切り詰め)
    for (let i = 0; i < 60 && monitor.bytes - beforeVim < RAW_RING_BYTES + 300_000; i++) {
      await typeKeys(client, paneId, '\x06'.repeat(30) + '\x02'.repeat(30));
      await sleep(150);
    }
    assert.ok(monitor.bytes - beforeVim > RAW_RING_BYTES, 'vim の出力だけで raw リングを超えた');
    await typeKeys(client, paneId, '20G');
    await assertJoinsConverge(paneId, 'b2');
    monitor.close();
    await typeKeys(client, paneId, '\x1b:q!\r');
    await client.request('pane.close', { paneId });
  });
});
