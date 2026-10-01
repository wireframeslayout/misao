import type { ParsedArgs, OptionSpecs } from '../args.js';
import type { CliIo } from '../cli-io.js';
import type { ResolvedConfig } from '../config/index.js';

export interface CommandContext {
  args: ParsedArgs;
  io: CliIo;
  isJson: boolean;
  config: ResolvedConfig;
}

export interface Command {
  name: string;
  /** 一覧用の 1 行説明 (日本語)。 */
  summary: string;
  usage: string;
  options: OptionSpecs;
  /** 終了コードを返す。失敗は CliError を投げる。 */
  run(ctx: CommandContext): Promise<number>;
}
