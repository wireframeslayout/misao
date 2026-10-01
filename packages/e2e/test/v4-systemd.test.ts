import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { after, before, test } from 'node:test';
import { MisaoClient } from '@misao/sdk';
import { fakeAgentCmd, openPane, paneInfo, screenText, typeKeys, waitForExit } from './helpers/agents.js';
import { assertHubLogComplete, assertPaneSurvived, snapshotPane } from './helpers/hub-survival.js';
import { waitForHubSubscribed } from './helpers/hub-process.js';
import { nodeCmd } from './helpers/node-cmd.js';
import { skipUnless } from './helpers/optin.js';
import { scale } from './helpers/scale.js';
import { deployUnitWithExecStart, formatExecStart, installUnit, killIfInUnit, mainPid, removeUnit, systemctl, uniqueUnitName } from './helpers/systemd-units.js';
import { sleep, waitFor } from './helpers/wait.js';

const OPTIONS = skipUnless('MISAO_E2E_SYSTEMD', 'systemd のユーザー unit を作る');
const CLI_MAIN = new URL('../../cli/src/main.ts', import.meta.url).pathname;
const FAKE_HUB = new URL('../src/fixtures/fake-hub.ts', import.meta.url).pathname;
const ROUNDS = scale(60, 400);
const DELAY_MS = 300;
const AGENT_ID = 'V4';
const RESTART_GRACE_MS = 3000;
const PLAN = scale<Array<'restart' | 'kill9'>>(['restart', 'kill9'], ['restart', 'restart', 'restart', 'restart', 'restart', 'kill9', 'kill9']);

const daemonUnit = uniqueUnitName('daemon');
const hubUnit = uniqueUnitName('hub');
let dir: string;
let socket: string;
let client: MisaoClient | undefined;
const panePids: number[] = [];

before(() => {
  if (OPTIONS.skip !== undefined) return;
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-e2e-sd-'));
  socket = path.join(dir, 'misao.sock');
});

after(() => {
  if (OPTIONS.skip !== undefined) return;
  client?.close();
  removeUnit(hubUnit);
  removeUnit(daemonUnit);
  // デーモンが SIGKILL で落ちるなどして pane が残った場合だけ、記録した PID を止める。
  // 正常時はデーモンが停止時に pane を閉じるので、終了済みの PID (再利用されうる) には送らない。
  for (const pid of panePids) killIfInUnit(pid, daemonUnit);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('v4: systemd ユーザー unit の hub を restart / kill -9 しても、デーモンと pane は生き残る', { ...OPTIONS, timeout: scale(180_000, 900_000) }, async () => {
  const stateFile = path.join(dir, 'hub-state.json');
  const logFile = path.join(dir, 'hub-log.ndjson');
  const summaryFile = path.join(dir, 'agent.summary');
  // 開発者の ~/.misao/misao.json を読ませないよう、一時ディレクトリの設定を明示する
  const configPath = path.join(dir, 'misao.json');
  fs.writeFileSync(configPath, JSON.stringify({ logLevel: 'warn' }));
  installUnit(daemonUnit, deployUnitWithExecStart(nodeCmd(CLI_MAIN, 'serve', '--config', configPath, '--socket', socket, '--data', dir)));
  installUnit(
    hubUnit,
    `[Unit]\nAfter=${daemonUnit}\n\n[Service]\nExecStart=${formatExecStart(nodeCmd(FAKE_HUB, '--socket', socket, '--state', stateFile, '--log', logFile))}\nRestart=always\nRestartSec=1\n`,
  );

  systemctl('start', daemonUnit);
  await waitFor(() => fs.existsSync(socket), 'the daemon socket', { timeoutMs: 15_000 });
  const daemonPid = mainPid(daemonUnit);
  assert.ok(daemonPid > 0, 'デーモンの unit が起動している');
  assert.equal(systemctl('show', '-p', 'KillMode', '--value', daemonUnit), 'process', 'deploy/misao.service の KillMode が効いている');
  assert.equal(systemctl('show', '-p', 'Restart', '--value', daemonUnit), 'on-failure', 'deploy/misao.service の Restart が効いている');
  client = new MisaoClient({ socketPath: socket });
  await client.connect();

  const bash = await openPane(client, ['bash', '--norc']);
  const agent = await openPane(client, fakeAgentCmd('markers', '--count', String(ROUNDS), '--id', AGENT_ID, '--delay', String(DELAY_MS), '--out', summaryFile));
  for (const paneId of [bash, agent]) panePids.push((await paneInfo(client, paneId)).pid!);
  await typeKeys(client, bash, 'echo BASH_ALIVE_V4\r');
  await waitFor(async () => (await screenText(client!, bash)).includes('BASH_ALIVE_V4'), 'bash to echo');
  systemctl('start', hubUnit);
  await waitForHubSubscribed(logFile, 1);

  for (const [index, how] of PLAN.entries()) {
    const before = { bash: await snapshotPane(client, bash), agent: await snapshotPane(client, agent) };
    const hubBefore = mainPid(hubUnit);
    if (how === 'restart') systemctl('restart', hubUnit);
    else killIfInUnit(hubBefore, hubUnit); // 記録した hub の PID で、その unit のプロセスのときだけ
    await waitFor(() => {
      const pid = mainPid(hubUnit);
      return pid > 0 && pid !== hubBefore;
    }, 'systemd to start a new hub', { timeoutMs: 20_000, stepMs: 200 });
    await waitForHubSubscribed(logFile, index + 2);

    assert.equal(systemctl('is-active', hubUnit), 'active', `${how}: hub の unit が active`);
    assert.equal(mainPid(daemonUnit), daemonPid, `${how}: デーモンの MainPID が同じ`);
    assertPaneSurvived(how, 'bash', before.bash, await snapshotPane(client, bash), true);
    assertPaneSurvived(how, 'agent', before.agent, await snapshotPane(client, agent), false);
  }

  await waitForExit(client, agent, scale(120_000, 6 * 60_000));
  await assertHubLogComplete(client, { logFile, agentPaneId: agent, agentId: AGENT_ID, summaryFile, killCount: PLAN.filter((how) => how === 'kill9').length });

  // 正常な stop は失敗ではないので、Restart=on-failure でも再起動されない (RestartSec=1 より長く待つ)
  systemctl('stop', hubUnit);
  systemctl('stop', daemonUnit);
  await sleep(RESTART_GRACE_MS);
  assert.equal(systemctl('is-active', daemonUnit), 'inactive', 'stop したデーモンは再起動しない');
  assert.equal(mainPid(daemonUnit), 0, 'stop したデーモンのプロセスが残っていない');
});
