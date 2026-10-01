import { parseKeyValues } from '../args.js';
import { withDaemon } from '../connect.js';
import { CliError } from '../errors.js';
import { writeJson, writeLine } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao label <target> [k=v ...] [--unset k ...] [--json]';

export const labelCommand: Command = {
  name: 'label',
  summary: 'ペインのラベルを設定・削除し、変更後のラベルを表示する',
  usage: USAGE,
  options: { unset: { type: 'string', multiple: true } },
  async run({ args, io, isJson, config }) {
    const [target, ...pairs] = expectPositionals(args, 1, Infinity, USAGE);
    const unset = args.strings('unset');
    if (pairs.length === 0 && unset.length === 0) throw new CliError('usage', `使い方: ${USAGE}`);
    const set = parseKeyValues(pairs, 'ラベル');
    const { labels } = await withDaemon(config.socket, async (client) => {
      const pane = await resolvePane(client, target!, io.homeDir);
      return client.request('pane.set_label', { paneId: pane.paneId, set, unset });
    });
    if (isJson) writeJson(io, { labels });
    else for (const [k, v] of Object.entries(labels)) writeLine(io.stdout, `${k}=${v}`);
    return 0;
  },
};
