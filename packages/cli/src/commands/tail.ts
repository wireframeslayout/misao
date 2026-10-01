import { withDaemon } from '../connect.js';
import { follow, resolveStartPosition } from '../follow.js';
import { writeJsonLine, writeLine } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao tail <target> [--since SEQ [--epoch EPOCH]] [--json]';

export const tailCommand: Command = {
  name: 'tail',
  summary: 'ペインの出力を行単位で追い続ける（Ctrl-C で終了）',
  usage: USAGE,
  options: { since: { type: 'string' }, epoch: { type: 'string' } },
  async run({ args, io, isJson, config }) {
    const [target] = expectPositionals(args, 1, 1, USAGE);
    const since = args.int('since', 0);
    const epochArg = args.string('epoch');
    return withDaemon(config.socket, async (client) => {
      const pane = await resolvePane(client, target!, io.homeDir);
      const { epoch } = await client.request('server.info', {});
      // 指定が無ければ保持している分を最初から再生してから追う。
      const start = resolveStartPosition(io, since, epochArg, epoch) ?? { since: 0, epoch };
      return follow(io, client, {
        warnTruncated: since !== undefined,
        initialEpoch: epoch,
        subscribe: (c, epochOf) =>
          c.subscribeLines(
            pane.paneId,
            (line) => (isJson ? writeJsonLine(io, { ...line, epoch: epochOf() }) : writeLine(io.stdout, line.text)),
            start,
          ),
      });
    });
  },
};
