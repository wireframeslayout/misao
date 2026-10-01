import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { after, before, describe, test } from 'node:test';
import type { MisaoClient } from '@misao/sdk';
import { REPO_ROOT, cleanEnvPrefix, openPane, paneInfo, passTrustDialog, screenText, typeKeys } from './helpers/agents.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { skipUnless } from './helpers/optin.js';
import { waitFor, waitQuiet } from './helpers/wait.js';

/** detach の prefix 候補になりうる制御キー。pane に素通しで送っても、アプリが落ちないことを確かめる。 */
const KEYS: Array<[string, string]> = [
  ['Ctrl-] (0x1d)', '\x1d'],
  ['Ctrl-\\ (0x1c)', '\x1c'],
  ['Ctrl-^ (0x1e)', '\x1e'],
  ['Ctrl-_ (0x1f)', '\x1f'],
];

interface App {
  name: string;
  cmd: (dir: string) => string[];
  setup: (client: MisaoClient, paneId: string) => Promise<void>;
  options: { skip: string } | { skip?: undefined };
}

const APPS: App[] = [
  {
    name: 'vim',
    cmd: (dir) => ['vim', '-u', 'NONE', '-N', path.join(dir, 'words.txt')],
    setup: async (client, paneId) => {
      await waitFor(async () => (await client.request('pane.screen', { paneId })).altScreen, 'vim to enter the alt screen');
      await typeKeys(client, paneId, 'w');
    },
    options: {},
  },
  {
    name: 'bash (readline)',
    cmd: () => ['bash', '--norc', '-i'],
    setup: async (client, paneId) => {
      await typeKeys(client, paneId, 'echo hello');
      await waitFor(async () => (await screenText(client, paneId)).includes('echo hello'), 'the typed text to appear');
    },
    options: {},
  },
  {
    name: 'claude (haiku, idle)',
    cmd: () => [...cleanEnvPrefix(), 'claude', '--model', 'claude-haiku-4-5-20251001'],
    setup: (client, paneId) => passTrustDialog(client, paneId),
    options: skipUnless('MISAO_E2E_CLAUDE', 'Claude Code (認証と課金が要る) を使う'),
  },
  {
    name: 'codex (idle)',
    cmd: () => [...cleanEnvPrefix(), 'codex'],
    setup: async () => undefined,
    options: skipUnless('MISAO_E2E_CODEX', 'Codex (認証が要る) を使う'),
  },
];

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
  fs.writeFileSync(path.join(daemon.dir, 'words.txt'), 'alpha beta gamma\ndelta epsilon zeta\n');
});
after(async () => {
  await daemon.stop();
});

describe('v5: detach キー候補の素通し', () => {
  for (const app of APPS) {
    test(`${app.name} は制御キーを受けても落ちない`, { ...app.options, timeout: 90_000 }, async () => {
      const paneId = await openPane(client, app.cmd(daemon.dir), { cwd: REPO_ROOT });
      await app.setup(client, paneId);
      await waitQuiet(client, paneId, { quietMs: 1000, timeoutMs: 30_000 });
      const started = await paneInfo(client, paneId);
      assert.equal(started.processState, 'running');
      for (const [label, keys] of KEYS) {
        await typeKeys(client, paneId, keys);
        await waitQuiet(client, paneId, { quietMs: 500, timeoutMs: 15_000 });
        const info = await paneInfo(client, paneId);
        assert.deepEqual(
          { state: info.processState, pid: info.pid, exitCode: info.exitCode, signal: info.signal },
          { state: 'running', pid: started.pid, exitCode: null, signal: null },
          `${label} の後も同じプロセスが動いている`,
        );
      }
      await client.request('pane.close', { paneId });
    });
  }
});
