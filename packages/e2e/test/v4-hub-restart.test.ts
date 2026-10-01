import assert from 'node:assert/strict';
import * as path from 'node:path';
import { after, before, test } from 'node:test';
import type { MisaoClient } from '@misao/sdk';
import { fakeAgentCmd, openPane, screenText, typeKeys, waitForExit } from './helpers/agents.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { assertHubLogComplete, assertPaneSurvived, snapshotPane } from './helpers/hub-survival.js';
import { startHubProcess, waitForHubSubscribed } from './helpers/hub-process.js';
import type { HubProcess } from './helpers/hub-process.js';
import { scale } from './helpers/scale.js';
import { waitFor } from './helpers/wait.js';

type Restart = 'restart' | 'kill9';

const ROUNDS = scale(60, 400);
const DELAY_MS = 300;
const PLAN = scale<Restart[]>(['restart', 'kill9', 'restart'], ['restart', 'restart', 'restart', 'restart', 'restart', 'kill9', 'kill9']);

let daemon: DaemonProcess;
let client: MisaoClient;
let hub: HubProcess | undefined;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(async () => {
  await hub?.stop();
  await daemon.stop();
});

test('v4: hub を再起動 / kill -9 してもデーモンと pane は生き残り、hub は保存位置から取りこぼしなく再開する', { timeout: scale(180_000, 900_000) }, async () => {
  const stateFile = path.join(daemon.dir, 'hub-state.json');
  const logFile = path.join(daemon.dir, 'hub-log.ndjson');
  const summaryFile = path.join(daemon.dir, 'agent.summary');
  const startHub = (): HubProcess => startHubProcess({ socket: daemon.socket, stateFile, logFile });

  const bash = await openPane(client, ['bash', '--norc']);
  const agent = await openPane(client, fakeAgentCmd('markers', '--count', String(ROUNDS), '--id', 'V4', '--delay', String(DELAY_MS), '--out', summaryFile));
  await typeKeys(client, bash, 'echo BASH_ALIVE_V4\r');
  await waitFor(async () => (await screenText(client, bash)).includes('BASH_ALIVE_V4'), 'bash to echo');
  hub = startHub();
  await waitForHubSubscribed(logFile, 1);

  for (const [index, how] of PLAN.entries()) {
    const before = { bash: await snapshotPane(client, bash), agent: await snapshotPane(client, agent) };
    if (how === 'restart') await hub.stop();
    else await hub.kill();
    hub = startHub();
    await waitForHubSubscribed(logFile, index + 2);

    assert.equal((await client.request('server.info', {})).pid, daemon.pid, `${how}: デーモンの PID が同じ`);
    assertPaneSurvived(how, 'bash', before.bash, await snapshotPane(client, bash), true);
    assertPaneSurvived(how, 'agent', before.agent, await snapshotPane(client, agent), false);
  }

  await waitForExit(client, agent, scale(120_000, 6 * 60_000));
  await assertHubLogComplete(client, { logFile, agentPaneId: agent, summaryFile, killCount: PLAN.filter((how) => how === 'kill9').length });
});
