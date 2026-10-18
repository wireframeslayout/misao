// 使い方: node scripts/smoke-binary.mjs <misao-x.y.z-<target>.tar.gz | .zip>
// Rust 版のアーカイブを展開し、中身を確かめたうえで、--version、serve (一時ソケット)、server.info (status --json)、
// pane の作成・出力・kill、`misao stop` による停止まで通す。
// Linux は静的リンク、Windows は C ランタイムの DLL に依存しないことも確かめる。
// 期待する版とターゲットはファイル名から取り、--version と server.info.version が完全に一致することを確かめる。
// 常駐デーモンには触れない: HOME・USERPROFILE・MISAO_DIR・MISAO_SOCKET はすべて一時ディレクトリに向ける。
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { PROTOCOL_VERSION } from '../packages/protocol/src/version.ts';
import { extractArchive, listArchive } from './release/archive.mjs';
import { forbiddenImports, readPeImports } from './release/pe-imports.mjs';
import {
  BINARY_TARGETS,
  binaryArchiveDir,
  binaryArchiveMembers,
  binaryExecutableName,
  isWindowsTarget,
} from './release/manifest.mjs';

const [archiveArg] = process.argv.slice(2);
if (archiveArg === undefined) throw new Error('usage: node scripts/smoke-binary.mjs <misao-x.y.z-<target>.tar.gz|.zip>');
const archive = path.resolve(archiveArg);

function parseArchiveName(file) {
  for (const { target } of BINARY_TARGETS) {
    const match = new RegExp(`^misao-(.+)-${target}\\.(?:tar\\.gz|zip)$`).exec(path.basename(file));
    if (match !== null) return { version: match[1], target };
  }
  throw new Error(`archive file name must be misao-<version>-<target>.tar.gz|.zip: ${file}`);
}
const { version: expectedVersion, target } = parseArchiveName(archive);
const windows = isWindowsTarget(target);
const PANE_MARKER = 'misao-smoke-pane-ok';
const START_TIMEOUT_MS = 20_000;
const STOP_TIMEOUT_MS = 20_000;

function runBinary(bin, args, env) {
  const result = spawnSync(bin, args, { encoding: 'utf8', env });
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

// アーカイブが、トップレベルのディレクトリ 1 つの下に、決められたファイルだけを持つこと。
function assertArchiveLayout() {
  const dir = binaryArchiveDir(expectedVersion, target);
  const expected = [dir, ...binaryArchiveMembers(target).map((member) => `${dir}/${member}`)].sort();
  const actual = [...new Set(listArchive({ archive, windows }).map((entry) => entry.replace(/^\.\//, '')))].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`archive entries are [${actual}], expected [${expected}]`);
  }
  console.log(`archive layout ok: ${dir}/{${binaryArchiveMembers(target).join(',')}}`);
  return dir;
}

// musl 向けのバイナリは静的リンクであること (glibc の最低バージョンに依存しない)。
function assertStaticLinux(bin) {
  if (process.platform !== 'linux' || !target.endsWith('-linux-musl')) return;
  const result = spawnSync('ldd', [bin], { encoding: 'utf8' });
  const text = `${result.stdout}${result.stderr}`;
  if (!/not a dynamic executable|statically linked/.test(text)) throw new Error(`${bin} is dynamically linked:\n${text}`);
  console.log('static link ok');
}

// Windows のバイナリは C ランタイムを静的リンクしている (VC++ 再頒布可能パッケージが無い PC でも動く) こと。
function assertNoRuntimeDlls(bin) {
  if (!windows) return;
  const dlls = readPeImports(readFileSync(bin));
  const forbidden = forbiddenImports(dlls);
  if (forbidden.length > 0) throw new Error(`${bin} imports the C runtime DLLs [${forbidden}] (all imports: ${dlls})`);
  console.log(`no C runtime DLL imports ok (imports: ${dlls.join(', ')})`);
}

// pane で走らせるコマンド: マーカーを出して待ち続ける。node はこのスクリプトを動かしているものを使う (全 OS にある)。
function paneCommand() {
  return [process.execPath, '-e', `console.log(${JSON.stringify(PANE_MARKER)});setTimeout(()=>{},60000)`];
}

const base = realpathSync(mkdtempSync(path.join(os.tmpdir(), 'misao-smoke-')));
let daemon;
try {
  const dirName = assertArchiveLayout();
  const extracted = path.join(base, 'extract');
  mkdirSync(extracted);
  extractArchive({ archive, dest: extracted, windows });
  const bin = path.join(extracted, dirName, binaryExecutableName(target));
  if (!existsSync(bin)) throw new Error(`${bin} not found after extraction`);
  assertStaticLinux(bin);
  assertNoRuntimeDlls(bin);

  const home = path.join(base, 'home');
  const dir = path.join(base, 'run');
  mkdirSync(home);
  mkdirSync(dir, { mode: 0o700 });
  const socket = windows ? `\\\\.\\pipe\\misao-smoke-${process.pid}` : path.join(dir, 'misao.sock');
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('MISAO_')));
  Object.assign(env, { HOME: home, USERPROFILE: home, MISAO_DIR: dir, MISAO_SOCKET: socket });

  const version = runBinary(bin, ['--version'], env);
  if (version.status !== 0 || version.stdout.trim() !== `misao ${expectedVersion}`) {
    throw new Error(`--version expected "misao ${expectedVersion}" (status ${version.status}): ${version.stdout}${version.stderr}`);
  }
  console.log(`--version ok: ${version.stdout.trim()}`);

  daemon = spawn(bin, ['serve'], { env, stdio: ['ignore', 'ignore', 'pipe'] });
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
        const status = runBinary(bin, ['status', '--json'], env);
        return status.status === 0 ? JSON.parse(status.stdout) : undefined;
      },
      START_TIMEOUT_MS,
      `server.info (stderr: ${stderr})`,
    ),
  ]);
  if (info.running !== true || info.pid !== daemon.pid) throw new Error(`unexpected server.info: ${JSON.stringify(info)}`);
  if (info.version !== expectedVersion) throw new Error(`server.info version is ${info.version}, expected ${expectedVersion}`);
  if (info.protocolVersion !== PROTOCOL_VERSION) throw new Error(`protocolVersion ${info.protocolVersion}, expected ${PROTOCOL_VERSION}`);
  console.log(`server.info ok: pid ${info.pid}, version ${info.version}, protocol ${info.protocolVersion}`);

  const created = runBinary(bin, ['new', '--json', '--cwd', base, '--', ...paneCommand()], env);
  if (created.status !== 0) throw new Error(`new failed (status ${created.status}): ${created.stdout}${created.stderr}`);
  const { paneId } = JSON.parse(created.stdout);
  await waitFor(
    () => {
      const screen = runBinary(bin, ['screen', paneId, '--json'], env);
      return screen.status === 0 && JSON.parse(screen.stdout).text.includes(PANE_MARKER);
    },
    START_TIMEOUT_MS,
    `pane output "${PANE_MARKER}"`,
  );
  const killed = runBinary(bin, ['kill', paneId, '--force'], env);
  if (killed.status !== 0) throw new Error(`kill failed (status ${killed.status}): ${killed.stdout}${killed.stderr}`);
  console.log(`pane ok: ${paneId} created, printed, killed`);

  // 停止も同じバイナリで行う。Windows には届けられるシグナルが無いので、全 OS で `misao stop` を使う。
  const stopped = runBinary(bin, ['stop'], env);
  if (stopped.status !== 0) throw new Error(`stop failed (status ${stopped.status}): ${stopped.stdout}${stopped.stderr}`);
  const result = await Promise.race([exited, new Promise((resolve) => setTimeout(() => resolve('timeout'), STOP_TIMEOUT_MS))]);
  if (result === 'timeout') throw new Error('daemon did not stop after `misao stop`');
  if (result.code !== 0) throw new Error(`daemon exited with ${JSON.stringify(result)}: ${stderr}`);
  console.log('stop ok');
} finally {
  if (daemon !== undefined && daemon.exitCode === null && daemon.signalCode === null) daemon.kill('SIGKILL');
  rmSync(base, { recursive: true, force: true });
}
console.log(`binary smoke test passed (${target})`);
