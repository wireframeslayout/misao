// 使い方: node scripts/smoke-bundle.mjs <misao-x.y.z.mjs>
// バンドルを一時ディレクトリに置き、node-pty を隣の node_modules として見せた状態で
// --version、serve（一時ソケット）、server.info（status --json）、停止まで確かめる。
// 常駐デーモンには触れない: HOME・MISAO_DIR・MISAO_SOCKET はすべて一時ディレクトリに向ける。
import { spawn, spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { PROTOCOL_VERSION } from '../packages/protocol/src/version.ts';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const [bundleArg] = process.argv.slice(2);
if (bundleArg === undefined) throw new Error('usage: node scripts/smoke-bundle.mjs <misao-x.y.z.mjs>');
const bundleSource = path.resolve(bundleArg);

const START_TIMEOUT_MS = 15_000;
const STOP_TIMEOUT_MS = 15_000;

function runBundle(bundle, args, env) {
  const result = spawnSync(process.execPath, [bundle, ...args], { encoding: 'utf8', env });
  if (result.error) throw result.error;
  return result;
}

async function waitFor(check, timeoutMs, what) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

const base = realpathSync(mkdtempSync(path.join(os.tmpdir(), 'misao-smoke-')));
let daemon;
try {
  // node-pty は同梱側が持つ前提。リポジトリの node_modules を隣に見せる。
  mkdirSync(path.join(base, 'node_modules'));
  symlinkSync(realpathSync(path.join(root, 'node_modules/node-pty')), path.join(base, 'node_modules/node-pty'));
  const bundle = path.join(base, path.basename(bundleSource));
  copyFileSync(bundleSource, bundle);

  const home = path.join(base, 'home');
  const dir = path.join(base, 'run');
  mkdirSync(home);
  const socket = path.join(dir, 'misao.sock');
  const env = { PATH: process.env.PATH, HOME: home, MISAO_DIR: dir, MISAO_SOCKET: socket };

  const version = runBundle(bundle, ['--version'], env);
  if (version.status !== 0 || !/^misao /.test(version.stdout)) {
    throw new Error(`--version failed (status ${version.status}): ${version.stdout}${version.stderr}`);
  }
  console.log(`--version ok: ${version.stdout.trim()}`);

  mkdirSync(dir, { mode: 0o700 });
  daemon = spawn(process.execPath, [bundle, 'serve'], { env, stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  daemon.stderr.on('data', (chunk) => (stderr += chunk));
  const exited = new Promise((resolve) => daemon.once('exit', (code, signal) => resolve({ code, signal })));
  const failIfExited = exited.then((r) => {
    throw new Error(`daemon exited early (${JSON.stringify(r)}): ${stderr}`);
  });

  const info = await Promise.race([
    failIfExited,
    waitFor(
      () => {
        const status = runBundle(bundle, ['status', '--json'], env);
        return status.status === 0 ? JSON.parse(status.stdout) : undefined;
      },
      START_TIMEOUT_MS,
      `server.info (stderr: ${stderr})`,
    ),
  ]);
  if (info.running !== true || info.pid !== daemon.pid) {
    throw new Error(`unexpected server.info: ${JSON.stringify(info)}`);
  }
  if (info.protocolVersion !== PROTOCOL_VERSION) {
    throw new Error(`protocolVersion ${info.protocolVersion}, expected ${PROTOCOL_VERSION}`);
  }
  console.log(`server.info ok: pid ${info.pid}, protocol ${info.protocolVersion}`);

  // 起動した PID にだけ SIGTERM を送る。
  daemon.kill('SIGTERM');
  const result = await Promise.race([
    exited,
    new Promise((resolve) => setTimeout(() => resolve('timeout'), STOP_TIMEOUT_MS)),
  ]);
  if (result === 'timeout') throw new Error('daemon did not stop after SIGTERM');
  if (result.code !== 0) throw new Error(`daemon exited with ${JSON.stringify(result)}: ${stderr}`);
  console.log('stop ok');
} finally {
  if (daemon !== undefined && daemon.exitCode === null && daemon.signalCode === null) daemon.kill('SIGKILL');
  rmSync(base, { recursive: true, force: true });
}
console.log('bundle smoke test passed');
