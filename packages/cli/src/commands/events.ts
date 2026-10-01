import { withDaemon } from '../connect.js';
import { follow, resolveStartPosition } from '../follow.js';
import { writeJsonLine, writeLine } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao events [--since SEQ [--epoch EPOCH]] [--pane <target>] [--json]';

export const eventsCommand: Command = {
  name: 'events',
  summary: 'デーモンのイベントを追い続ける（Ctrl-C で終了）',
  usage: USAGE,
  options: { since: { type: 'string' }, epoch: { type: 'string' }, pane: { type: 'string' } },
  async run({ args, io, isJson, config }) {
    expectPositionals(args, 0, 0, USAGE);
    const since = args.int('since', 0);
    const epochArg = args.string('epoch');
    const paneQuery = args.string('pane');
    return withDaemon(config.socket, async (client) => {
      const paneId = paneQuery === undefined ? undefined : (await resolvePane(client, paneQuery, io.homeDir)).paneId;
      const { epoch } = await client.request('server.info', {});
      // 指定が無ければライブのみ。
      const start = resolveStartPosition(io, since, epochArg, epoch);
      return follow(io, client, {
        warnTruncated: since !== undefined,
        initialEpoch: epoch,
        subscribe: (c, epochOf) =>
          c.subscribeEvents(
            (event) => {
              if (paneId !== undefined && event.paneId !== paneId) return;
              if (isJson) writeJsonLine(io, { ...event, epoch: epochOf() });
              else writeLine(io.stdout, `${event.seq}  ${event.ts}  ${event.type}  ${event.paneId ?? '-'}  ${JSON.stringify(event.data)}`);
            },
            start ?? {},
          ),
      });
    });
  },
};
