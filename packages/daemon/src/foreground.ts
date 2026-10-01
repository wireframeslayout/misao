import { readFileSync } from 'node:fs';
import * as path from 'node:path';

/** インタプリタの引数の読み方。スクリプト名を探すときに飛ばす・止まる引数を決める。 */
interface InterpreterSyntax {
  /** 次の引数を値として取るオプション (`--opt=value` の形なら次の引数は飛ばさない)。 */
  valueOptions: ReadonlySet<string>;
  /** コードを直接渡すオプション。スクリプトが無いのでインタプリタ名を返す。 */
  codeOptions: ReadonlySet<string>;
  /** 次の引数のモジュール名をそのまま名前にするオプション (python -m)。 */
  moduleOptions: ReadonlySet<string>;
  /** スクリプトの前に置くサブコマンド (bun run)。 */
  subcommands: ReadonlySet<string>;
}

const NODE: InterpreterSyntax = {
  valueOptions: new Set(['-r', '--require', '--import', '--loader', '--experimental-loader', '-C', '--conditions', '--env-file', '--title']),
  codeOptions: new Set(['-e', '--eval', '-p', '--print']),
  moduleOptions: new Set(),
  subcommands: new Set(),
};
const BUN: InterpreterSyntax = {
  valueOptions: new Set(['-r', '--require', '--preload', '--import', '--conditions', '--cwd', '-c', '--config', '--env-file']),
  codeOptions: new Set(['-e', '--eval', '-p', '--print']),
  moduleOptions: new Set(),
  subcommands: new Set(['run', 'x']),
};
const PYTHON: InterpreterSyntax = {
  valueOptions: new Set(['-W', '-X']),
  codeOptions: new Set(['-c']),
  moduleOptions: new Set(['-m']),
  subcommands: new Set(),
};
const RUBY: InterpreterSyntax = {
  valueOptions: new Set(['-r', '-I', '-C']),
  codeOptions: new Set(['-e']),
  moduleOptions: new Set(),
  subcommands: new Set(),
};
const PERL: InterpreterSyntax = {
  valueOptions: new Set(),
  codeOptions: new Set(['-e', '-E']),
  moduleOptions: new Set(),
  subcommands: new Set(),
};

/** 前面のコマンド名として実体ではなくスクリプト名を見せたいインタプリタ。シェルは含めない。 */
const INTERPRETERS: readonly (readonly [RegExp, InterpreterSyntax])[] = [
  [/^(node|nodejs)$/, NODE],
  [/^bun$/, BUN],
  [/^python(3(\.\d+)?)?$/, PYTHON],
  [/^ruby$/, RUBY],
  [/^perl$/, PERL],
];

/** 表示用の名前の上限 (コードポイント数)。 */
const MAX_COMMAND_NAME_LENGTH = 64;
/** 制御文字・書式文字 (bidi・ゼロ幅など)・行区切り・段落区切り。端末や表に流れても表示を乱さないよう取り除く。 */
const UNSAFE_CHARS = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;
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

/** 表示用に制御文字・書式文字を除き、64 コードポイントで切る。空になれば undefined。 */
export function sanitizeCommandName(name: string): string | undefined {
  const cleaned = [...name.replace(UNSAFE_CHARS, '')].slice(0, MAX_COMMAND_NAME_LENGTH).join('');
  return cleaned === '' ? undefined : cleaned;
}

/** argv から表示用のコマンド名を決める。インタプリタ経由ならスクリプト名 (拡張子なし) かモジュール名。 */
export function commandNameFromArgv(argv: readonly string[]): string | undefined {
  const [first] = argv;
  if (first === undefined) return undefined;
  const command = path.basename(first);
  const syntax = INTERPRETERS.find(([pattern]) => pattern.test(command))?.[1];
  return sanitizeCommandName(syntax === undefined ? command : scriptName(command, argv.slice(1), syntax));
}

/** インタプリタの引数からスクリプト名を探す。スクリプトが無ければインタプリタ名。 */
function scriptName(command: string, args: readonly string[], syntax: InterpreterSyntax): string {
  let sawSubcommand = false;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (arg === '--') return args[i + 1] === undefined ? command : scriptBasename(args[i + 1]!);
    // --opt=value の形は名前と値に分ける
    const name = arg.startsWith('--') ? arg.split('=', 1)[0]! : arg;
    const inlineValue = name === arg ? undefined : arg.slice(name.length + 1);
    if (syntax.codeOptions.has(name)) return command;
    if (syntax.moduleOptions.has(name)) return inlineValue ?? args[i + 1] ?? command;
    if (syntax.valueOptions.has(name)) {
      if (inlineValue === undefined) i++;
      continue;
    }
    if (arg.startsWith('-')) continue;
    if (!sawSubcommand && syntax.subcommands.has(arg)) {
      sawSubcommand = true;
      continue;
    }
    return scriptBasename(arg);
  }
  return command;
}

function scriptBasename(script: string): string {
  return path.parse(path.basename(script)).name;
}

/**
 * 前面プロセスのコマンド名。Linux は pty の前面プロセスグループのリーダーの cmdline から、
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
    // 子の終了や入れ替わり (リーダーが先に終わったパイプラインなど) で /proc が消えるのは普通に起きる。前面コマンドは表示用なので省略で足りる
    return undefined;
  }
}
