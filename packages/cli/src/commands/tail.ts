import { withDaemon } from '../connect.js';
import { follow } from '../follow.js';
import { writeJsonLine, writeLine } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao tail <target> [--since SEQ] [--json]';

export const tailCommand: Command = {
  name: 'tail',
  summary: 'ペインの出力を行単位で追い続ける（Ctrl-C で終了）',
  usage: USAGE,
  options: { since: { type: 'string' } },
  async run({ args, io, isJson, config }) {
    const [target] = expectPositionals(args, 1, 1, USAGE);
    const since = args.int('since', 0);
    return withDaemon(config.socket, async (client) => {
      const pane = await resolvePane(client, target!, io.homeDir);
      // since は epoch と組で渡す。指定が無ければ保持している分を最初から再生してから追う。
      const { epoch } = await client.request('server.info', {});
      return follow(io, client, {
        warnTruncated: since !== undefined,
        subscribe: (c) =>
          c.subscribeLines(
            pane.paneId,
            (line) => (isJson ? writeJsonLine(io, line) : writeLine(io.stdout, line.text)),
            { since: since ?? 0, epoch },
          ),
      });
    });
  },
};
