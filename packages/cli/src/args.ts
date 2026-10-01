import { parseArgs } from 'node:util';
import { CliError } from './errors.js';

export interface OptionSpec {
  type: 'boolean' | 'string';
  multiple?: boolean;
}

export type OptionSpecs = Readonly<Record<string, OptionSpec>>;

/** どのコマンドでも受け付ける。 */
export const GLOBAL_OPTIONS = {
  config: { type: 'string' },
  json: { type: 'boolean' },
  help: { type: 'boolean' },
  version: { type: 'boolean' },
} as const satisfies OptionSpecs;

type RawValue = string | boolean | Array<string | boolean> | undefined;

/** 解析済みの引数。値の取り出しで型と範囲を検証する (境界での入力検証)。 */
export class ParsedArgs {
  constructor(
    /** `--` より前の位置引数。 */
    readonly positionals: readonly string[],
    /** `--` より後ろ。 */
    readonly rest: readonly string[],
    private readonly values: Readonly<Record<string, RawValue>>,
  ) {}

  flag(name: string): boolean {
    return this.values[name] === true;
  }

  string(name: string): string | undefined {
    const v = this.values[name];
    return typeof v === 'string' ? v : undefined;
  }

  strings(name: string): string[] {
    const v = this.values[name];
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  }

  /** 整数オプション。min 未満や数字以外は usage エラー。 */
  int(name: string, min: number): number | undefined {
    const raw = this.string(name);
    if (raw === undefined) return undefined;
    if (!/^\d+$/.test(raw) || Number(raw) < min) {
      throw new CliError('usage', `--${name} には ${min} 以上の整数を指定してください: ${raw}`);
    }
    return Number(raw);
  }
}

function usageError(error: unknown): CliError {
  return new CliError('usage', `引数が正しくありません: ${error instanceof Error ? error.message : String(error)}`);
}

export function parseCommandArgs(argv: readonly string[], specs: OptionSpecs): ParsedArgs {
  try {
    const { values, tokens } = parseArgs({
      args: [...argv],
      options: { ...specs, ...GLOBAL_OPTIONS },
      allowPositionals: true,
      strict: true,
      tokens: true,
    });
    const terminator = tokens.find((t) => t.kind === 'option-terminator')?.index;
    const positionals = tokens
      .filter((t) => t.kind === 'positional' && (terminator === undefined || t.index < terminator))
      .map((t) => (t.kind === 'positional' ? t.value : ''));
    const rest = terminator === undefined ? [] : argv.slice(terminator + 1);
    return new ParsedArgs(positionals, rest, values);
  } catch (error) {
    throw usageError(error);
  }
}

export interface SplitArgs {
  /** 最初の位置引数。無ければ undefined。 */
  command: string | undefined;
  /** command を除いた引数。 */
  args: string[];
  /** コマンド名より前後どこにあっても読めるグローバルオプション。 */
  globals: ParsedArgs;
}

/** コマンド名とグローバルオプションを先に取り出す (コマンド固有のオプションはまだ解釈しない)。 */
export function splitCommand(argv: readonly string[]): SplitArgs {
  const { values, tokens } = parseArgs({
    args: [...argv],
    options: GLOBAL_OPTIONS,
    allowPositionals: true,
    strict: false,
    tokens: true,
  });
  const first = tokens.find((t) => t.kind === 'positional');
  const args = first === undefined ? [...argv] : argv.filter((_, i) => i !== first.index);
  return {
    command: first?.kind === 'positional' ? first.value : undefined,
    args,
    globals: new ParsedArgs([], [], values),
  };
}
