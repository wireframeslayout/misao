import { readFileSync } from 'node:fs';
import * as path from 'node:path';

/** 表示用の名前の上限 (コードポイント数)。 */
const MAX_COMMAND_NAME_LENGTH = 64;
/** 前面のコマンド名として実体ではなくスクリプト名を見せたいインタプリタ。シェルは含めない。 */
const INTERPRETER = /^(node|nodejs|bun|python|python3(\.\d+)?|ruby|perl)$/;
/** C0・DEL・C1 の制御文字。端末へ流れても画面を乱さないよう取り除く。 */
const CONTROL_CHARS = /[\u0000-\u001f\u007f-\u009f]/g;
/** 非 Linux では /proc が無いので node-pty の値を使う。起動時に一度だけ決める。 */
const IS_LINUX = process.platform === 'linux';

/** /proc/<pid>/stat から前面プロセスグループ ID (tpgid) を取り出す。取れなければ undefined。 */
export function parseForegroundPgid(stat: string): number | undefined {
  // comm は括弧で囲まれ、中に ") (" などを含みうるので、最後の ")" より後ろを空白で分割する
  const rest = stat.slice(stat.lastIndexOf(')') + 1).trim().split(/\s+/);
  // 括弧の後ろは state, ppid, pgrp, session, tty_nr, tpgid の順
  const tpgid = Number(rest[5]);
  return Number.isInteger(tpgid) && tpgid > 0 ? tpgid : undefined;
}

/** 表示用に制御文字を除き、64 コードポイントで切る。空になれば undefined。 */
export function sanitizeCommandName(name: string): string | undefined {
  const cleaned = [...name.replace(CONTROL_CHARS, '')].slice(0, MAX_COMMAND_NAME_LENGTH).join('');
  return cleaned === '' ? undefined : cleaned;
}

/** argv から表示用のコマンド名を決める。インタプリタ経由なら最初のスクリプト引数の名前 (拡張子なし)。 */
export function commandNameFromArgv(argv: readonly string[]): string | undefined {
  const [first] = argv;
  if (first === undefined) return undefined;
  const command = path.basename(first);
  if (!INTERPRETER.test(command)) return sanitizeCommandName(command);
  const script = argv.slice(1).find((arg) => !arg.startsWith('-'));
  return sanitizeCommandName(script === undefined ? command : path.parse(path.basename(script)).name);
}

/**
 * 前面プロセスのコマンド名。Linux は pty の前面プロセスグループの cmdline から、
 * それ以外は node-pty の `process` から得る。取れなければ undefined (応答は落とさない)。
 */
export function readForegroundCommand(pid: number, ptyProcess: () => string): string | undefined {
  try {
    if (!IS_LINUX) return sanitizeCommandName(ptyProcess());
    const pgid = parseForegroundPgid(readFileSync(`/proc/${pid}/stat`, 'utf8'));
    if (pgid === undefined) return undefined;
    const argv = readFileSync(`/proc/${pgid}/cmdline`, 'utf8').split('\0');
    if (argv.at(-1) === '') argv.pop();
    return commandNameFromArgv(argv);
  } catch {
    // 子の終了や入れ替わりで /proc が消えるのは普通に起きる。前面コマンドは表示用なので省略で足りる
    return undefined;
  }
}
