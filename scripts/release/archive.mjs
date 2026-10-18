import path from 'node:path';
import { run } from './exec.mjs';

// Rust 版バイナリのアーカイブの作成・一覧・展開。Unix は tar.gz、Windows は zip。
// Windows は OS 標準の bsdtar (System32\tar.exe) を使う。PATH の先頭に Git の GNU tar があっても zip を扱えるように、絶対パスで呼ぶ。
function tarCommand(windows) {
  if (!windows) return 'tar';
  const systemRoot = process.env.SystemRoot;
  if (systemRoot === undefined || systemRoot === '') throw new Error('SystemRoot is not set');
  return path.join(systemRoot, 'System32', 'tar.exe');
}

// dir (cwd の直下のディレクトリ) を archive にまとめる。macOS の tar が付ける AppleDouble (._*) は無効にする。
export function createArchive({ archive, cwd, dir, windows }) {
  const flags = windows ? ['-a', '-c', '-f'] : ['-czf'];
  run(tarCommand(windows), [...flags, archive, '-C', cwd, dir], { env: { COPYFILE_DISABLE: '1' } });
}

// archive の中のパス (ディレクトリ末尾の / は除く)。
export function listArchive({ archive, windows }) {
  const out = run(tarCommand(windows), ['-tf', archive], { capture: true });
  return out
    .split(/\r?\n/)
    .filter(Boolean)
    .map((entry) => entry.replace(/\/$/, ''));
}

export function extractArchive({ archive, dest, windows }) {
  run(tarCommand(windows), ['-xf', archive, '-C', dest]);
}
