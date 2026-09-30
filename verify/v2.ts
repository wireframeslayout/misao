// v2: 複数 attach / replay 一致 / サイズ競合。結果は verify/results/v2.json
import * as fs from 'node:fs';
import * as path from 'node:path';
import { MisaoClient } from '../src/client/MisaoClient.js';
import {
  ClientView, cleanEnvPrefix, closeAll, compareScreens, monitor, open, paneInfo, sleep, startDaemon, waitFor,
  waitQuiet, writeResult, ROOT, passTrustDialog,
} from './lib.js';

const COLS = 100;
const ROWS = 30;
type Replay = 'raw' | 'snapshot';
const REPLAYS: Replay[] = ['raw', 'snapshot'];

const result: Record<string, unknown> = { startedAt: new Date().toISOString(), cases: {}, sizeConflict: {} };
const cases = result.cases as Record<string, unknown>;

async function screenOf(c: MisaoClient, paneId: string): Promise<string> {
  return (await c.request<{ text: string }>('pane.screen', { paneId })).text;
}

async function joinAndCompare(socket: string, c: MisaoClient, paneId: string, replay: Replay, tag: string) {
  const v = await ClientView.attach(socket, paneId, `join-${tag}-${replay}`, replay, { cols: COLS, rows: ROWS });
  await sleep(1200);
  await waitQuiet(c, paneId, 600, 20000);
  const cmp = compareScreens(await v.text(), await screenOf(c, paneId));
  const out = { replay, viewBytes: v.bytes, ...cmp };
  v.close();
  return out;
}

async function typeKeys(c: MisaoClient, paneId: string, s: string): Promise<void> {
  await c.request('pane.write', { paneId, data: s, source: 'hub' });
}

async function main(): Promise<void> {
  const d = await startDaemon('v2');
  const c = await MisaoClient.connect(d.socket);
  const mon = await monitor(d.socket);
  result.daemonPid = d.pid;
  try {
    // ---- (a) bash 大量出力 (1MiB 超) 後に途中参加
    {
      const p = await open(c, ['bash', '--norc'], { cols: COLS, rows: ROWS });
      await mon.client.request('pane.attach', { paneId: p, clientId: 'mon-a', replay: 'none' });
      await sleep(500);
      const before = mon.bytes();
      await typeKeys(c, p, `seq 1 40000 | sed 's/$/ padding-text-to-lengthen-the-line-0123456789abcdef/'\r`);
      await waitFor(async () => mon.bytes() - before > 1_500_000, 30000);
      await waitQuiet(c, p, 1000);
      const total = mon.bytes() - before;
      const joins = [];
      for (const r of REPLAYS) joins.push(await joinAndCompare(d.socket, c, p, r, 'a'));
      cases.a_bash_bulk = { outputBytes: total, ringTruncated: total > 1024 * 1024, joins };
      await c.request('pane.close', { paneId: p });
    }

    // ---- (b) 1MiB 超の出力後に vim (alt screen) を起動し、編集中に途中参加
    {
      const big = path.join(d.dir, 'big.txt');
      fs.writeFileSync(big, Array.from({ length: 5000 }, (_, i) => `line ${i} ` + 'x'.repeat(50)).join('\n') + '\n');
      const p = await open(c, ['bash', '--norc'], { cols: COLS, rows: ROWS });
      await mon.client.request('pane.attach', { paneId: p, clientId: 'mon-b', replay: 'none' });
      await sleep(500);
      const b0 = mon.bytes();
      await typeKeys(c, p, `seq 1 40000 | sed 's/$/ padding-text-to-lengthen-the-line-0123456789abcdef/'\r`);
      await waitFor(async () => mon.bytes() - b0 > 1_500_000, 30000);
      await waitQuiet(c, p, 1000);
      const preVim = mon.bytes() - b0;
      const v0 = mon.bytes();
      await typeKeys(c, p, `vim -u NONE -N ${big}\r`);
      await sleep(1500);
      await typeKeys(c, p, '10Gihello edit\x1b:set number\r5G');
      await waitQuiet(c, p, 800);
      const info = await c.request<{ altScreen: boolean }>('pane.screen', { paneId: p });
      const joins = [];
      for (const r of REPLAYS) joins.push(await joinAndCompare(d.socket, c, p, r, 'b'));
      // b2: vim の再描画だけで raw リングを 1MiB 超にしてから参加 (alt screen 内で切り詰め)
      for (let i = 0; i < 40 && mon.bytes() - v0 < 1_300_000; i++) {
        await typeKeys(c, p, '\x06'.repeat(30) + '\x02'.repeat(30));
        await sleep(150);
      }
      await typeKeys(c, p, '20G');
      await waitQuiet(c, p, 1000);
      const vimBytes = mon.bytes() - v0;
      const joins2 = [];
      for (const r of REPLAYS) joins2.push(await joinAndCompare(d.socket, c, p, r, 'b2'));
      cases.b_vim_alt = { outputBeforeVim: preVim, altScreen: info.altScreen, joins };
      cases.b2_vim_alt_ring_truncated = { vimOutputBytes: vimBytes, ringTruncated: vimBytes > 1024 * 1024, joins: joins2 };
      await typeKeys(c, p, '\x1b:q!\r');
      await c.request('pane.close', { paneId: p });
    }

    // ---- (c) Claude Code (haiku)。応答表示中に参加 → 完了後にも参加
    {
      const cwd = ROOT;
      const p = await open(
        c,
        [...cleanEnvPrefix(), 'claude', '--model', 'claude-haiku-4-5-20251001', 'Count from 1 to 30, one number per line.'],
        { cols: COLS, rows: ROWS, cwd },
      );
      const info0 = await paneInfo(c, p);
      const mid: Record<Replay, ClientView> = {} as never;
      const trustSeen = await passTrustDialog(c, p);
      const trustHandled = trustSeen ? [trustSeen] : [];
      await sleep(500);
      for (const r of REPLAYS) mid[r] = await ClientView.attach(d.socket, p, `mid-${r}`, r, { cols: COLS, rows: ROWS });
      const midScreenSample = (await screenOf(c, p)).split('\n').filter(Boolean).slice(-6);
      const done = await waitFor(async () => /\n\s*⏺?\s*30\b|\b30\s*$/m.test(await screenOf(c, p)), 90000, 1000);
      await waitQuiet(c, p, 3000, 30000);
      const finalScreen = await screenOf(c, p);
      const midResults = [];
      for (const r of REPLAYS) {
        midResults.push({ replay: r, joinedDuring: 'response', viewBytes: mid[r].bytes, ...compareScreens(await mid[r].text(), finalScreen) });
        mid[r].close();
      }
      const after = [];
      for (const r of REPLAYS) after.push(await joinAndCompare(d.socket, c, p, r, 'c-after'));
      cases.c_claude = { trustDialogSeen: trustHandled, responseCompleted: done, midScreenSample, midJoinFinal: midResults, joinAfter: after, cwd, paneAtStart: info0.cols };
      await c.request('pane.close', { paneId: p });
    }

    // ---- サイズ競合 (vim と claude)
    for (const app of ['vim', 'claude'] as const) {
      const cmd = app === 'vim'
        ? ['vim', '-u', 'NONE', '-N', path.join(d.dir, 'big.txt')]
        : [...cleanEnvPrefix(), 'claude', '--model', 'claude-haiku-4-5-20251001'];
      const p = await open(c, cmd, { cols: 100, rows: 30, cwd: ROOT });
      await sleep(app === 'claude' ? 5000 : 1500);
      if (app === 'claude') await passTrustDialog(c, p);
      const A = { cols: 120, rows: 40 };
      const B = { cols: 80, rows: 24 };
      const steps: unknown[] = [];
      const seenResized = () => mon.resized.filter((e) => e.paneId === p).length;
      let cursor = seenResized();
      let vA: ClientView | undefined;
      let vB: ClientView | undefined;
      const key = app === 'vim' ? 'l' : '\x0c'; // vim: 右移動 / claude: Ctrl-L (再描画)
      const record = async (label: string, expectSize: { cols: number; rows: number }, expectOwner: string) => {
        await sleep(1200);
        await waitQuiet(c, p, 500, 8000);
        const info = await paneInfo(c, p);
        const daemonText = await screenOf(c, p);
        const evs = mon.resized.filter((e) => e.paneId === p).slice(cursor);
        cursor = seenResized();
        const views: Record<string, unknown> = {};
        for (const [name, v] of [['A', vA], ['B', vB]] as const) {
          if (!v) continue;
          const cmp = compareScreens(await v.text(), daemonText, info.cols, info.rows);
          views[name] = { viewSize: `${v.cols}x${v.rows}`, matchRate: cmp.matchRate, mismatches: cmp.mismatches.slice(0, 3) };
        }
        steps.push({
          step: label, cols: info.cols, rows: info.rows, sizeOwner: info.sizeOwner, clients: info.clients,
          resizedEvents: evs, views,
          ok: info.cols === expectSize.cols && info.rows === expectSize.rows && info.sizeOwner === expectOwner,
        });
      };
      vA = await ClientView.attach(d.socket, p, 'A', 'snapshot', A, { announceSize: true });
      await record('1 A attach (120x40)', A, 'A');
      vB = await ClientView.attach(d.socket, p, 'B', 'snapshot', B, { announceSize: true });
      await record('2 B attach (80x24)', B, 'B');
      await vA.write(key);
      await record('3 A input', A, 'A');
      await vB.write(key);
      await record('4 B input', B, 'B');
      await vB.client.request('pane.detach', { paneId: p });
      vB.close();
      const bView = vB;
      vB = undefined;
      void bView;
      await record('5 B detach (owner leaves -> A remains)', A, 'A');
      (result.sizeConflict as Record<string, unknown>)[app] = { allStepsOk: steps.every((s) => (s as { ok: boolean }).ok), steps };
      vA.close();
      await c.request('pane.close', { paneId: p });
    }
  } finally {
    await closeAll(c).catch(() => undefined);
    c.close();
    mon.client.close();
    await d.stop();
  }
  result.finishedAt = new Date().toISOString();
  writeResult('v2', result);
}

main().catch((e) => {
  result.error = String(e?.stack ?? e);
  writeResult('v2', result);
  process.exit(1);
});
