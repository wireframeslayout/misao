// v5: hub 停止中の CLI 操作と追いつき / detach キー衝突。結果は verify/results/v5.json
import { spawn, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as pty from 'node-pty';
import { MisaoClient } from '../src/client/MisaoClient.js';
import { TTY_RESET } from '../src/cli/attach.js';
import { cleanEnvPrefix, closeAll, open, paneInfo, passTrustDialog, sleep, startDaemon, waitFor, waitQuiet, writeResult, ROOT } from './lib.js';

const result: Record<string, unknown> = { startedAt: new Date().toISOString() };

function startHub(dir: string, state: string, log: string, tag: string): ChildProcess {
  return spawn(process.execPath, [path.join(ROOT, 'dist/fixtures/fake-hub.js'), '--dir', dir, '--state', state, '--log', log, '--tag', tag], {
    env: { ...process.env, MISAO_SPIKE_TAG: tag }, stdio: 'ignore',
  });
}
async function stopChild(ch: ChildProcess): Promise<void> {
  if (ch.exitCode !== null || ch.signalCode !== null) return;
  const done = new Promise<void>((r) => ch.once('exit', () => r()));
  ch.kill('SIGTERM');
  await Promise.race([done, sleep(3000)]);
  if (ch.exitCode === null && ch.signalCode === null) ch.kill('SIGKILL');
}
const readLog = (f: string) => (fs.existsSync(f) ? fs.readFileSync(f, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l)) : []);

async function catchUp(d: { dir: string; socket: string }, c: MisaoClient): Promise<void> {
  const state = path.join(d.dir, 'hub-state.json');
  const log1 = path.join(d.dir, 'hub-log1.ndjson');
  const log2 = path.join(d.dir, 'hub-log2.ndjson');
  const q = await open(c, [process.execPath, path.join(ROOT, 'dist/fixtures/fake-agent.js'), 'question', '--id', 'Q5']);
  await sleep(800);
  const hub1 = startHub(d.dir, state, log1, 'verify-v5-hub1');
  await sleep(2000);
  await stopChild(hub1);
  const saved = JSON.parse(fs.readFileSync(state, 'utf8')) as { events: number; lines: Record<string, number>; epoch: string };

  // hub 停止中に CLI attach (PTY 内) で回答して detach
  const p = pty.spawn(process.execPath, [path.join(ROOT, 'dist/cli.js'), '--dir', d.dir, 'attach', q.slice(-8)], {
    name: 'xterm-256color', cols: 100, rows: 30, cwd: ROOT, env: { ...process.env, MISAO_SPIKE_TAG: 'verify-v5-attach' } as Record<string, string>,
  });
  let out = '';
  p.onData((s) => (out += s));
  const exit = new Promise<{ exitCode: number; signal?: number }>((r) => p.onExit(r));
  await waitFor(() => out.includes('QUESTION'), 8000, 100);
  p.write('blue\r');
  const answered = await waitFor(() => out.includes('ANSWER:blue'), 8000, 100);
  await sleep(500);
  p.write('\x1d');
  p.write('d');
  const ex = await Promise.race([exit, sleep(8000).then(() => undefined)]);
  const resetSeen = out.includes(TTY_RESET);
  if (!ex) p.kill();

  // hub 再起動 → 保存 seq から追いつく
  const hub2 = startHub(d.dir, state, log2, 'verify-v5-hub2');
  await sleep(3000);
  await stopChild(hub2);
  const log = readLog(log2);
  const subs = log.filter((m) => m.kind === 'subscribed');
  const lines = log.filter((m) => m.method === 'pane.line' && m.params.paneId === q);
  const events = log.filter((m) => m.method === 'event');
  const inputs = events.filter((m) => m.params.type === 'input').map((m) => m.params.data as Record<string, unknown>);
  const contig = (seqs: number[], start: number) => seqs.every((s, i) => s === start + 1 + i);
  const evSeqs = events.map((m) => m.params.seq as number);
  const lnSeqs = lines.map((m) => m.params.seq as number);
  const lineTexts = lines.map((m) => m.params.text as string);
  result.catchUp = {
    savedState: saved,
    attachAnswered: answered, cliExitCode: ex?.exitCode ?? null, cliExited: !!ex, ttyResetOutput: resetSeen,
    subscribeResults: subs.map((s) => ({ stream: s.stream, since: s.since, gap: s.gap, head: s.head })),
    answerLineSeen: lineTexts.includes('ANSWER:blue'), doneMarkerSeen: lineTexts.some((t) => /^AZITO_DONE_Q5_[0-9a-f]+$/.test(t)),
    inputEvents: inputs, inputEventKeys: [...new Set(inputs.flatMap((i) => Object.keys(i)))].sort(),
    inputSourceAllTerminal: inputs.length > 0 && inputs.every((i) => i.source === 'terminal'),
    inputContentLeaked: JSON.stringify(inputs).includes('blue'),
    eventSeqContiguousFromSaved: contig(evSeqs, saved.events), lineSeqContiguousFromSaved: contig(lnSeqs, saved.lines[q] ?? 0),
    eventSeqs: evSeqs, lineSeqs: lnSeqs, lineTexts,
  };
  const cu = result.catchUp as Record<string, unknown>;
  cu.pass = answered && ex?.exitCode === 0 && resetSeen && cu.answerLineSeen && cu.doneMarkerSeen && cu.inputSourceAllTerminal &&
    !cu.inputContentLeaked && cu.eventSeqContiguousFromSaved && cu.lineSeqContiguousFromSaved && subs.every((s) => s.gap === false);
  await c.request('pane.close', { paneId: q });
}

const KEYS: Array<[string, string]> = [['Ctrl-] (0x1d)', '\x1d'], ['Ctrl-\\ (0x1c)', '\x1c'], ['Ctrl-^ (0x1e)', '\x1e'], ['Ctrl-_ (0x1f)', '\x1f']];

async function keyCollisions(d: { dir: string; socket: string }, c: MisaoClient): Promise<void> {
  const apps: Array<{ name: string; cmd: string[]; setup: (p: string) => Promise<void> }> = [
    {
      name: 'vim',
      cmd: ['vim', '-u', 'NONE', '-N', path.join(d.dir, 'words.txt')],
      setup: async (p) => { await sleep(1200); await c.request('pane.write', { paneId: p, data: 'w', source: 'hub' }); await sleep(300); },
    },
    { name: 'bash(readline)', cmd: ['bash', '--norc', '-i'], setup: async (p) => { await c.request('pane.write', { paneId: p, data: 'echo hello', source: 'hub' }); await sleep(800); } },
    {
      name: 'claude(haiku, idle)',
      cmd: [...cleanEnvPrefix(), 'claude', '--model', 'claude-haiku-4-5-20251001'],
      setup: async (p) => { await passTrustDialog(c, p); await sleep(3000); },
    },
    { name: 'codex(idle)', cmd: [...cleanEnvPrefix(), 'codex'], setup: async () => { await sleep(6000); } },
  ];
  fs.writeFileSync(path.join(d.dir, 'words.txt'), 'alpha beta gamma\ndelta epsilon zeta\n');
  const table: unknown[] = [];
  for (const app of apps) {
    const p = await open(c, app.cmd, { cols: 100, rows: 30, cwd: ROOT });
    await app.setup(p);
    await waitQuiet(c, p, 1000, 15000);
    const startScreen = (await c.request<{ text: string }>('pane.screen', { paneId: p })).text;
    const row: Record<string, unknown> = { app: app.name, startScreenSample: startScreen.split('\n').filter(Boolean).slice(0, 4).join(' | ').slice(0, 300), keys: {} };
    for (const [label, seq] of KEYS) {
      const before = (await c.request<{ text: string }>('pane.screen', { paneId: p })).text;
      const info0 = await paneInfo(c, p);
      await c.request('pane.write', { paneId: p, data: seq, source: 'hub' }).catch(() => undefined);
      await sleep(1000);
      const info = await paneInfo(c, p);
      const after = (await c.request<{ text: string }>('pane.screen', { paneId: p })).text;
      const a = before.split('\n');
      const b = after.split('\n');
      const changed = b.map((l, i) => [i, a[i] ?? '', l] as const).filter(([, x, y]) => x.trimEnd() !== y.trimEnd());
      (row.keys as Record<string, unknown>)[label] = {
        alive: info.state === 'running' && info.pid === info0.pid, exitCode: info.exitCode, signal: info.signal,
        changedRows: changed.length, diffSample: changed.slice(0, 3).map(([i, x, y]) => `r${i}: "${x.trim().slice(0, 60)}" -> "${y.trim().slice(0, 60)}"`),
      };
    }
    row.finalAlive = (await paneInfo(c, p)).state === 'running';
    table.push(row);
    await c.request('pane.close', { paneId: p }).catch(() => undefined);
  }
  result.keyCollisions = table;
}

async function main(): Promise<void> {
  const d = await startDaemon('v5');
  const c = await MisaoClient.connect(d.socket);
  result.daemonPid = d.pid;
  try {
    await catchUp(d, c);
    await keyCollisions(d, c);
  } catch (e) {
    result.error = String((e as Error)?.stack ?? e);
  } finally {
    await closeAll(c).catch(() => undefined);
    c.close();
    await d.stop();
    result.finishedAt = new Date().toISOString();
    writeResult('v5', result);
  }
}
void main();
