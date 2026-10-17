import { fileURLToPath } from 'node:url';

/** Windows (ConPTY / 名前付きパイプ) で動かしているか。 */
export const IS_WINDOWS = process.platform === 'win32';

/** `new URL(relative, import.meta.url)` をファイルパスにする（Windows でも使える）。 */
export function filePath(relative: string, base: string): string {
  return fileURLToPath(new URL(relative, base));
}

/** POSIX のシェル・vim・systemd などに頼るテストを、Windows でだけ skip する指定。 */
export function skipOnWindows(reason: string): { skip: string } | { skip?: undefined } {
  return IS_WINDOWS ? { skip: `${reason} (POSIX 前提のため Windows では skip)` } : {};
}

/** node で JavaScript を実行する cmd。Windows でシェルの代わりに、同じ動作を書くのに使う。 */
export function nodeEval(script: string): string[] {
  return [process.execPath, '-e', script];
}

/** 長く動き続けるだけの cmd（約 5 分）。Windows の `ping` は孫プロセスになる。 */
export function sleepCmd(): string[] {
  return IS_WINDOWS ? ['ping', '-n', '300', '127.0.0.1'] : ['sleep', '300'];
}

/** 対話シェルの cmd。 */
export function interactiveShell(): string[] {
  return IS_WINDOWS ? ['cmd.exe'] : ['bash', '--norc'];
}
