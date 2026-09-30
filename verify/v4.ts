// v4: hub (fake-hub) を再起動/kill -9 してもデーモンとペインが生き残るか (systemd user unit)。結果は verify/results/v4.json
import { execFileSync, spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { MisaoClient } from '../src/client/MisaoClient.js';
import { open, paneInfo, sleep, waitFor, writeResult, ROOT } from './lib.js';

const DAEMON_UNIT = 'misao-spike-v4.service';
const HUB_UNIT = 'misao-spike-v4-hub.service';
const UNIT_DIR = path.join(os.homedir(), '.config/systemd/user');
const RUN = path.join(ROOT, '.run/v4');
const ROUNDS = 400;
const DELAY = 300;

const sc = (...args: string[]): string => {
  const r = spawnSync('systemctl', ['--user', ...args], { encoding: 'utf8' });
  return (r.stdout + r.stderr).trim();
};
const mainPid = (unit: string): number => Number(sc('show', '-p', 'MainPID', '--value', unit));

function render(tpl: string, unit: string): string {
  return fs
    .readFileSync(path.join(ROOT, 'deploy', tpl), 'utf8')
    .replaceAll('<NODE>', process.execPath)
    .replaceAll('<REPO>/.run', RUN)
    .replaceAll('<REPO>', ROOT)
    .replace('After=misao-spike.service', `After=${DAEMON_UNIT}`)
    .replace('Restart=always', 'Restart=on-failure')
    .replace('misao-spike.service', unit);
}

interface Snap { pid: number; state: string; screen: string; }
async function snap(c: MisaoClient, paneId: string): Promise<Snap> {
  const i = await paneInfo(c, paneId);
  const s = await c.request<{ text: string }>('pane.screen', { paneId });
  return { pid: i.pid, state: i.state, screen: s.text };
}

const result: Record<string, unknown> = { startedAt: new Date().toISOString() };

async function main(): Promise<void> {
  fs.rmSync(RUN, { recursive: true, force: true });
  fs.mkdirSync(RUN, { recursive: true });
  fs.mkdirSync(UNIT_DIR, { recursive: true });
  fs.writeFileSync(path.join(UNIT_DIR, DAEMON_UNIT), render('misao-spike.service.template', DAEMON_UNIT));
  fs.writeFileSync(path.join(UNIT_DIR, HUB_UNIT), render('misao-spike-hub.service.template', HUB_UNIT));
  sc('daemon-reload');
  let c: MisaoClient | undefined;
  try {
    sc('start', DAEMON_UNIT);
    await waitFor(() => fs.existsSync(path.join(RUN, 'misao.sock')), 10000);
    const daemonPid = mainPid(DAEMON_UNIT);
    c = await MisaoClient.connect(path.join(RUN, 'misao.sock'));
    const bash = await open(c, ['bash', '--norc']);
    const agent = await open(c, [
      process.execPath, path.join(ROOT, 'dist/fixtures/fake-agent.js'), 'markers', '--count', String(ROUNDS), '--id', 'V4',
      '--delay', String(DELAY), '--out', path.join(RUN, 'agent.summary'),
    ]);
    await c.request('pane.write', { paneId: bash, data: 'echo BASH_ALIVE_V4\r', source: 'hub' });
    sc('start', HUB_UNIT);
    await sleep(2500);

    const rounds: unknown[] = [];
    const hubPids: number[] = [];
    const plan = [...Array(5).fill('restart'), ...Array(2).fill('kill9')] as Array<'restart' | 'kill9'>;
    for (const [i, how] of plan.entries()) {
      const before = { bash: await snap(c, bash), agent: await snap(c, agent) };
      const hubBefore = mainPid(HUB_UNIT);
      hubPids.push(hubBefore);
      if (how === 'restart') sc('restart', HUB_UNIT);
      else process.kill(hubBefore, 'SIGKILL');
      await waitFor(() => { const p = mainPid(HUB_UNIT); return p > 0 && p !== hubBefore; }, 20000, 200);
      await sleep(3000);
      const after = { bash: await snap(c, bash), agent: await snap(c, agent) };
      rounds.push({
        n: i + 1, how, hubPidBefore: hubBefore, hubPidAfter: mainPid(HUB_UNIT), hubActive: sc('is-active', HUB_UNIT),
        daemonPidSame: mainPid(DAEMON_UNIT) === daemonPid,
        bashPidSame: before.bash.pid === after.bash.pid, agentPidSame: before.agent.pid === after.agent.pid,
        bashScreenSame: before.bash.screen === after.bash.screen, bashRunning: after.bash.state === 'running',
        agentRunning: after.agent.state === 'running', agentScreenNonEmpty: after.agent.screen.trim().length > 0,
      });
      await sleep(2000);
    }

    // agent 完走を待って、hub ログから取り逃し / 重複 / seq 連続性を検証
    const finished = await waitFor(() => fs.existsSync(path.join(RUN, 'agent.summary')), 6 * 60_000, 1000);
    await sleep(4000);
    const summary = fs.existsSync(path.join(RUN, 'agent.summary'))
      ? (JSON.parse(fs.readFileSync(path.join(RUN, 'agent.summary'), 'utf8').replace(/^FAKE_SUMMARY /, '')) as { nonces: string[] })
      : { nonces: [] };
    const expected = summary.nonces.map((n) => `AZITO_DONE_V4_${n}`);
    const log = fs.readFileSync(path.join(RUN, 'hub-log.ndjson'), 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
    const lineSeqs: number[] = [];
    const evSeqs: number[] = [];
    const found = new Map<string, number>();
    for (const m of log) {
      if (m.method === 'pane.line' && m.params.paneId === agent) {
        lineSeqs.push(m.params.seq);
        for (const x of (m.params.text as string).matchAll(/AZITO_DONE_[A-Za-z0-9]+_[A-Za-z0-9]+/g)) found.set(x[0], (found.get(x[0]) ?? 0) + 1);
      } else if (m.method === 'event') evSeqs.push(m.params.seq);
    }
    const analyze = (seqs: number[]) => {
      const dups = seqs.length - new Set(seqs).size;
      const uniq = [...new Set(seqs)].sort((a, b) => a - b);
      let holes = 0;
      for (let i = 1; i < uniq.length; i++) holes += uniq[i]! - uniq[i - 1]! - 1;
      return { count: seqs.length, duplicates: dups, holes, first: uniq[0], last: uniq[uniq.length - 1], monotonicInArrival: seqs.every((s, i) => i === 0 || s >= seqs[i - 1]!) };
    };
    result.rounds = rounds;
    result.daemonPid = daemonPid;
    result.agentFinished = finished;
    result.markers = {
      expected: expected.length,
      missed: expected.filter((e) => !found.has(e)).length,
      duplicated: [...found.entries()].filter(([, v]) => v > 1).length,
      falsePositives: [...found.keys()].filter((k) => !expected.includes(k)).length,
    };
    result.lineSeq = analyze(lineSeqs);
    result.eventSeq = analyze(evSeqs);
    result.hubLogKinds = log.filter((m) => m.kind).reduce((a: Record<string, number>, m) => ((a[m.kind] = (a[m.kind] ?? 0) + 1), a), {});
    const allRoundsOk = (rounds as Array<Record<string, boolean | string>>).every(
      (r) => r.daemonPidSame && r.bashPidSame && r.agentPidSame && r.bashScreenSame && r.bashRunning && r.agentRunning && r.hubActive === 'active',
    );
    result.pass = allRoundsOk && finished && (result.markers as { missed: number }).missed === 0 && (result.lineSeq as { holes: number; duplicates: number }).holes === 0 && (result.lineSeq as { duplicates: number }).duplicates === 0;
  } finally {
    // 後始末: unit 停止/無効化/削除 → 残存プロセス確認
    c?.close();
    sc('stop', HUB_UNIT);
    sc('stop', DAEMON_UNIT);
    sc('disable', HUB_UNIT);
    sc('disable', DAEMON_UNIT);
    fs.rmSync(path.join(UNIT_DIR, DAEMON_UNIT), { force: true });
    fs.rmSync(path.join(UNIT_DIR, HUB_UNIT), { force: true });
    sc('daemon-reload');
    sc('reset-failed');
    await sleep(2000);
    const left = spawnSync('pgrep', ['-af', `${RUN}|fake-agent.js markers.*--id V4|fake-hub.js.*v4`], { encoding: 'utf8' }).stdout.trim();
    result.cleanup = { unitFilesExist: [DAEMON_UNIT, HUB_UNIT].map((u) => fs.existsSync(path.join(UNIT_DIR, u))), leftoverProcesses: left ? left.split('\n').filter((l) => !l.includes('pgrep')) : [], units: sc('list-units', '--all', 'misao-spike-v4*') };
    result.finishedAt = new Date().toISOString();
    writeResult('v4', result);
  }
}
void execFileSync;
main().catch((e) => { console.error(e); process.exit(1); });
