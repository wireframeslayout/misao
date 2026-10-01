import { splitCommand, parseCommandArgs } from './args.js';
import type { CliIo } from './cli-io.js';
import { COMMANDS } from './commands/index.js';
import { loadConfig } from './config/index.js';
import { CliError, toCliError } from './errors.js';
import { PACKAGE } from './index.js';
import { renderError, writeLine } from './output.js';
import { TARGET_HELP } from './target.js';

const GLOBAL_HELP = [
  '--config PATH  設定ファイル (misao.json) を指定する',
  '--json         出力を JSON にする',
  '--help         このヘルプを表示する',
  '--version      バージョンを表示する',
];

function helpText(): string {
  const width = Math.max(...COMMANDS.map((c) => c.name.length));
  const lines = COMMANDS.map((c) => `  ${c.name.padEnd(width)}  ${c.summary}`);
  return [
    '使い方: misao [オプション] <コマンド> [引数]',
    '',
    'コマンド:',
    ...lines,
    '',
    'オプション:',
    ...GLOBAL_HELP.map((l) => `  ${l}`),
    '',
    ...TARGET_HELP,
  ].join('\n');
}

function commandHelpText(usage: string, summary: string): string {
  const lines = [`使い方: ${usage}`, summary];
  if (usage.includes('<target>')) lines.push('', ...TARGET_HELP);
  return lines.join('\n');
}

/** argv を解釈してコマンドを実行し、終了コードを返す。process.exit はエントリ (main.ts) だけが呼ぶ。 */
export async function run(argv: readonly string[], io: CliIo): Promise<number> {
  const { command: name, args, globals } = splitCommand(argv);
  const isJson = globals.flag('json');
  if (globals.flag('version')) {
    writeLine(io.stdout, `${PACKAGE} 0.0.0`);
    return 0;
  }
  const command = COMMANDS.find((c) => c.name === name);
  if (globals.flag('help') && command === undefined) {
    writeLine(io.stdout, helpText());
    return 0;
  }
  try {
    if (name === undefined) throw new CliError('usage', `コマンドを指定してください\n${helpText()}`);
    if (command === undefined) throw new CliError('usage', `不明なコマンドです: ${name}\n${helpText()}`);
    const parsed = parseCommandArgs(args, command.options);
    if (parsed.flag('help')) {
      writeLine(io.stdout, commandHelpText(command.usage, command.summary));
      return 0;
    }
    const loaded = loadConfig({ flagPath: parsed.string('config'), env: io.env, homeDir: io.homeDir });
    for (const w of loaded.warnings) writeLine(io.stderr, `[misao] 警告: ${w}`);
    return await command.run({ args: parsed, io, isJson, config: loaded.config });
  } catch (error) {
    const cliError = toCliError(error);
    renderError(io, cliError, isJson);
    return cliError.exitCode;
  }
}
