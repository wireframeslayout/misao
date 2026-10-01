import { after, before, test } from 'node:test';
import type { MisaoClient } from '@misao/sdk';
import { REPO_ROOT, cleanEnvPrefix, openPane, passTrustDialog, screenText } from './helpers/agents.js';
import { ClientView } from './helpers/client-view.js';
import type { Replay } from './helpers/client-view.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { skipUnless } from './helpers/optin.js';
import { runSizeConflict } from './helpers/size-conflict.js';
import { waitFor, waitQuiet } from './helpers/wait.js';

const OPTIONS = skipUnless('MISAO_E2E_CLAUDE', 'Claude Code (認証と課金が要る) を使う');
const MODEL = ['--model', 'claude-haiku-4-5-20251001'];
const REPLAYS: Replay[] = ['raw', 'snapshot'];

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  if (OPTIONS.skip !== undefined) return;
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(async () => {
  if (OPTIONS.skip === undefined) await daemon.stop();
});

test('v2: Claude Code の応答中に参加した視点も、完了後に参加した視点も最終画面に一致する', { ...OPTIONS, timeout: 180_000 }, async () => {
  const paneId = await openPane(client, [...cleanEnvPrefix(), 'claude', ...MODEL, 'Count from 1 to 30, one number per line.'], { cwd: REPO_ROOT });
  await passTrustDialog(client, paneId);
  const midViews = await Promise.all(REPLAYS.map((replay) => ClientView.attach(daemon.socket, paneId, `mid-${replay}`, replay, { cols: 100, rows: 30 })));
  try {
    await waitFor(async () => /\n\s*⏺?\s*30\b|\b30\s*$/m.test(await screenText(client, paneId)), 'the response to reach 30', { timeoutMs: 90_000, stepMs: 1000 });
    await waitQuiet(client, paneId, { quietMs: 3000 });
    for (const view of midViews) await view.waitForMatch(client);
    for (const replay of REPLAYS) {
      const view = await ClientView.attach(daemon.socket, paneId, `after-${replay}`, replay, { cols: 100, rows: 30 });
      try {
        await view.waitForMatch(client);
      } finally {
        view.close();
      }
    }
  } finally {
    for (const view of midViews) view.close();
  }
  await client.request('pane.close', { paneId });
});

test('v2: Claude Code でも pane サイズの所有者が入力した側へ移る', { ...OPTIONS, timeout: 120_000 }, async () => {
  const paneId = await openPane(client, [...cleanEnvPrefix(), 'claude', ...MODEL], { cwd: REPO_ROOT });
  await passTrustDialog(client, paneId);
  await waitQuiet(client, paneId, { quietMs: 2000 });
  await runSizeConflict(daemon.socket, client, paneId, '\x0c'); // Ctrl-L (再描画)
  await client.request('pane.close', { paneId });
});
