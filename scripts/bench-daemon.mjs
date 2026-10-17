// デーモンのメモリ（RSS）と CPU の比較計測（#32 の条件）。Node 版と Rust 版に、同じペイン数・同じ scrollback・同じ出力の負荷をかける。
//
//   npm run build && cargo build --release -p misao
//   node scripts/bench-daemon.mjs [--impl node,rust] [--panes 10,30,60] [--patterns quiet,bulk,tui]
//                                 [--scrollback 5000] [--rust-bin target/release/misao] [--tui-seconds 20] [--json]
//
// 計測の方法:
//  - 一時ディレクトリに misao.json を置き、`misao serve` を子プロセスで起動する（~/.misao のデーモンには触れない）。
//  - RSS は /proc/<pid>/status の VmRSS（ピークは VmHWM）。CPU は /proc/<pid>/stat の utime + stime（デーモン 1 プロセスだけ。pane の子は含まない）。
//  - 負荷: quiet = 出力なし（sleep）/ bulk = 各 pane が 6 MB の行（100000 行）を一気に出す / tui = Claude Code 風の 12 行の画面を 10 回/秒で再描画する。
//  - 負荷が終わった時点（bulk は出力が止まってから 1 秒後、tui は --tui-seconds 秒後）で RSS を読む。tui の CPU はその時間の平均。
// Linux 専用（/proc を読む）。

import { spawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { MisaoClient } from '../packages/sdk-node/dist/index.js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const option = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const list = (name, fallback) => option(name, fallback).split(',').filter(Boolean);

const impls = list('--impl', 'node,rust');
const paneCounts = list('--panes', '10,30,60').map(Number);
const patterns = list('--patterns', 'quiet,bulk,tui');
const scrollback = Number(option('--scrollback', '5000'));
const rawBytes = option('--raw-bytes') === undefined ? undefined : Number(option('--raw-bytes'));
const linesBytes = option('--lines-bytes') === undefined ? undefined : Number(option('--lines-bytes'));
const rustBin = path.resolve(root, option('--rust-bin', 'target/release/misao'));
const tuiSeconds = Number(option('--tui-seconds', '20'));
const subscribeLines = args.includes('--subscribe-lines');
const asJson = args.includes('--json');

const CLK_TCK = 100; // getconf CLK_TCK（Linux は事実上 100）
const BULK_LINES = 100_000;
const LINE = 'the quick brown fox jumps over the lazy dog 0123456789 abcdefghijkl';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function readStatus(pid) {
  const text = fs.readFileSync(`/proc/${pid}/status`, 'utf8');
  const kb = (key) => Number(new RegExp(`^${key}:\\s+(\\d+) kB`, 'm').exec(text)?.[1] ?? 0);
  return { rssMiB: kb('VmRSS') / 1024, peakMiB: kb('VmHWM') / 1024 };
}

function cpuSeconds(pid) {
  const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
  const fields = stat.slice(stat.lastIndexOf(')') + 2).split(' '); // state は 3 番目
  return (Number(fields[11]) + Number(fields[12])) / CLK_TCK;
}

const TUI_SCRIPT = `
import sys, time
frame = 0
spin = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
end = time.time() + ${tuiSeconds + 30}
sys.stdout.write("\\x1b[?2026h\\x1b[?25l")
while time.time() < end:
    out = ["\\x1b[H"]
    for row in range(12):
        out.append("\\x1b[2K\\x1b[38;5;%dm%s row %d frame %d\\x1b[0m" % (100 + row, spin[frame % 10], row, frame))
        out.append("\\r\\n" if row < 11 else "")
    sys.stdout.write("".join(out)); sys.stdout.flush()
    frame += 1
    time.sleep(0.1)
`;

function paneCommand(pattern) {
  switch (pattern) {
    case 'quiet':
      return ['sh', '-c', 'sleep 3600'];
    case 'bulk':
      return ['sh', '-c', `yes '${LINE}' | head -n ${BULK_LINES}; sleep 3600`];
    case 'tui':
      return ['python3', '-u', '-c', TUI_SCRIPT];
    default:
      throw new Error(`unknown pattern: ${pattern}`);
  }
}

async function startDaemon(impl, config) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-bench-'));
  const socket = path.join(dir, 'm.sock');
  const configPath = path.join(dir, 'misao.json');
  fs.writeFileSync(configPath, JSON.stringify({ ...config, logLevel: 'warn' }));
  const serve = ['serve', '--config', configPath, '--socket', socket, '--data', dir];
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('MISAO_')));
  const [command, ...rest] =
    impl === 'rust' ? [rustBin, ...serve] : [process.execPath, path.join(root, 'packages/cli/dist/main.js'), ...serve];
  const child = spawn(command, rest, { env, stdio: 'ignore' });
  for (let i = 0; i < 300 && !fs.existsSync(socket); i++) await sleep(50);
  if (!fs.existsSync(socket)) throw new Error(`${impl} daemon did not start`);
  const client = new MisaoClient({ socketPath: socket });
  await client.connect();
  return {
    pid: child.pid,
    client,
    async stop() {
      client.close();
      child.kill('SIGTERM');
      await new Promise((resolve) => child.once('exit', resolve));
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** 全 pane の出力が `quietMs` 動かなくなるまで待つ。 */
async function waitAllQuiet(client, paneIds, quietMs, timeoutMs) {
  const last = new Map();
  const changedAt = new Map();
  const start = Date.now();
  for (;;) {
    let quiet = true;
    for (const paneId of paneIds) {
      const { activity } = await client.request('pane.screen', { paneId });
      if (last.get(paneId) !== activity) {
        last.set(paneId, activity);
        changedAt.set(paneId, Date.now());
      }
      if (Date.now() - changedAt.get(paneId) < quietMs) quiet = false;
    }
    if (quiet) return;
    if (Date.now() - start > timeoutMs) throw new Error('timeout waiting for the panes to go quiet');
    await sleep(200);
  }
}

async function runOne(impl, panes, pattern) {
  const config = { scrollback, rings: { ...(rawBytes && { rawBytes }), ...(linesBytes && { linesBytes }) } };
  const daemon = await startDaemon(impl, config);
  try {
    await sleep(500);
    const idle = readStatus(daemon.pid);
    const cpu0 = cpuSeconds(daemon.pid);
    const t0 = Date.now();
    const cmd = paneCommand(pattern);
    const paneIds = [];
    for (let i = 0; i < panes; i++) {
      const { paneId } = await daemon.client.request('pane.open', { cmd, cols: 120, rows: 40 });
      paneIds.push(paneId);
      if (subscribeLines) await daemon.client.subscribeLines(paneId, () => undefined);
    }
    if (pattern === 'tui') await sleep(tuiSeconds * 1000);
    else if (pattern === 'bulk') await waitAllQuiet(daemon.client, paneIds, 1000, 300_000);
    else await sleep(2000);
    const wallSeconds = (Date.now() - t0) / 1000;
    const cpuSecondsUsed = cpuSeconds(daemon.pid) - cpu0;
    const loaded = readStatus(daemon.pid);
    return {
      impl,
      panes,
      pattern,
      scrollback,
      idleRssMiB: idle.rssMiB,
      rssMiB: loaded.rssMiB,
      peakRssMiB: loaded.peakMiB,
      rssPerPaneMiB: (loaded.rssMiB - idle.rssMiB) / panes,
      cpuSeconds: cpuSecondsUsed,
      wallSeconds,
      cpuPercent: (cpuSecondsUsed / wallSeconds) * 100,
    };
  } finally {
    await daemon.stop();
  }
}

const results = [];
for (const pattern of patterns) {
  for (const panes of paneCounts) {
    for (const impl of impls) {
      const result = await runOne(impl, panes, pattern);
      results.push(result);
      if (!asJson) {
        const f = (n) => n.toFixed(1).padStart(7);
        console.log(
          `${pattern.padEnd(5)} ${String(panes).padStart(3)} panes ${impl.padEnd(4)}  idle ${f(result.idleRssMiB)} MiB  rss ${f(result.rssMiB)} MiB  peak ${f(result.peakRssMiB)} MiB  per-pane ${f(result.rssPerPaneMiB)} MiB  cpu ${f(result.cpuSeconds)} s / ${f(result.wallSeconds)} s (${f(result.cpuPercent)} %)`,
        );
      }
    }
  }
}
if (asJson) console.log(JSON.stringify(results, null, 2));
process.exit(0);
