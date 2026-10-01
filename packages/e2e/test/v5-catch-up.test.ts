import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { after, before, test } from 'node:test';
import type { MisaoClient } from '@misao/sdk';
import { fakeAgentCmd, openPane } from './helpers/agents.js';
import { DETACH_KEY, PREFIX_KEY, startCliAttach } from './helpers/cli-attach.js';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';
import { readHubLog, startHubProcess, waitForHubSubscribed } from './helpers/hub-process.js';
import { waitFor } from './helpers/wait.js';

const PICKER_PROMPT = '番号で入る';
const PICKER_QUIT = 'q\r';
const TTY_CURSOR_SHOWN = '\x1b[?25h'; // detach 時の端末の復元 (TTY_RESET の一部)

let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(async () => {
  await daemon.stop();
});

function isContiguousFrom(seqs: number[], start: number): boolean {
  return seqs.every((seq, i) => seq === start + 1 + i);
}

test('v5: hub 停止中に CLI の attach で操作しても、再起動した hub は保存位置から取りこぼしなく追いつく', { timeout: 120_000 }, async () => {
  const stateFile = path.join(daemon.dir, 'hub-state.json');
  const firstLog = path.join(daemon.dir, 'hub-log1.ndjson');
  const secondLog = path.join(daemon.dir, 'hub-log2.ndjson');
  const paneId = await openPane(client, fakeAgentCmd('question', '--id', 'Q5'));

  // hub を一度走らせて位置を保存させ、止める
  const firstHub = startHubProcess({ socket: daemon.socket, stateFile, logFile: firstLog });
  await waitForHubSubscribed(firstLog, 1);
  await waitFor(() => readHubLog(firstLog).some((e) => e.kind === 'line' && e.paneId === paneId), 'the first hub to see a line');
  await firstHub.stop();
  const saved = JSON.parse(fs.readFileSync(stateFile, 'utf8')) as { events: { seq: number }; lines: Record<string, { seq: number }> };

  // hub 停止中に、疑似端末の CLI で pane へ入って回答し、detach で抜ける
  const attach = startCliAttach(daemon, paneId.slice(-8));
  try {
    await attach.waitForOutput('QUESTION');
    attach.write('blue\r');
    await attach.waitForOutput('ANSWER:blue');
    attach.write(PREFIX_KEY);
    attach.write(DETACH_KEY);
    await attach.waitForOutput(PICKER_PROMPT); // detach すると ペイン一覧に戻る
    attach.write(PICKER_QUIT);
    assert.equal(await attach.exited(), 0, '一覧で q を入力すると正常終了する');
    assert.ok(attach.output().includes(TTY_CURSOR_SHOWN), '端末の状態が復元される');
  } finally {
    attach.kill();
  }

  // hub を再起動し、保存位置から最新まで追いつくのを待つ
  const secondHub = startHubProcess({ socket: daemon.socket, stateFile, logFile: secondLog });
  try {
    await waitForHubSubscribed(secondLog, 1);
    const { eventHead } = await client.request('server.info', {});
    const { head } = await client.request('pane.subscribe_lines', { paneId });
    await waitFor(() => {
      const log = readHubLog(secondLog);
      return log.filter((e) => e.kind === 'event').at(-1)?.seq === eventHead && log.filter((e) => e.kind === 'line').at(-1)?.seq === head;
    }, 'the second hub to catch up');
  } finally {
    await secondHub.stop();
  }

  const log = readHubLog(secondLog);
  const events = log.filter((e) => e.kind === 'event');
  const lines = log.filter((e) => e.kind === 'line' && e.paneId === paneId);
  const inputs = events.filter((e) => e.type === 'input').map((e) => e.data!);
  assert.deepEqual(log.filter((e) => e.kind === 'gap'), [], '位置の保存が効いていて gap にならない');
  assert.ok(isContiguousFrom(events.map((e) => e.seq!), saved.events.seq), 'イベント seq が保存位置から連続している');
  assert.ok(isContiguousFrom(lines.map((e) => e.seq!), saved.lines[paneId]!.seq), '行 seq が保存位置から連続している');
  const lineTexts = lines.map((e) => e.text!);
  assert.ok(lineTexts.includes('ANSWER:blue'), '回答の行を受け取った');
  assert.ok(lineTexts.some((text) => /^AZITO_DONE_Q5_[0-9a-f]+$/.test(text)), '完了マーカーを受け取った');
  assert.ok(inputs.length > 0 && inputs.every((data) => data.source === 'terminal'), '入力イベントの source は terminal');
  assert.ok(!JSON.stringify(inputs).includes('blue'), '入力イベントに入力内容を含めない');
  await client.request('pane.close', { paneId });
});
