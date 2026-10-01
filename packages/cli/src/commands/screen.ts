import { withDaemon } from '../connect.js';
import { writeJson, writeLine } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao screen <target> [--lines N] [--json]';

/** 画面の末尾の空行を除いたうえで、末尾 lines 行だけを残す。 */
function tailLines(text: string, lines: number | undefined): string {
  const all = text.split('\n');
  while (all.length > 0 && all[all.length - 1]!.trim() === '') all.pop();
  return (lines === undefined ? all : all.slice(-lines)).join('\n');
}

export const screenCommand: Command = {
  name: 'screen',
  summary: 'ペインの現在の画面テキストを表示する',
  usage: USAGE,
  options: { lines: { type: 'string' } },
  async run({ args, io, isJson, config }) {
    const [target] = expectPositionals(args, 1, 1, USAGE);
    const lines = args.int('lines', 1);
    const screen = await withDaemon(config.socket, async (client) => {
      const pane = await resolvePane(client, target!, io.homeDir);
      return client.request('pane.screen', { paneId: pane.paneId });
    });
    const text = tailLines(screen.text, lines);
    if (isJson) writeJson(io, { ...screen, text });
    else writeLine(io.stdout, text);
    return 0;
  },
};
