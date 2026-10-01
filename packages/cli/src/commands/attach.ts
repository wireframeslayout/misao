import { runAttachLoop } from '../attach/loop.js';
import { requireTerminal } from '../attach/tty.js';
import { withDaemon } from '../connect.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao attach <target> [--readonly] [--no-replay]';

export const attachCommand: Command = {
  name: 'attach',
  summary: 'ペインへ入る（prefix + d で抜ける）',
  usage: USAGE,
  options: { readonly: { type: 'boolean' }, 'no-replay': { type: 'boolean' } },
  async run({ args, io, config }) {
    const [target] = expectPositionals(args, 1, 1, USAGE);
    requireTerminal(io); // 端末でなければ、接続の前に使い方の誤りとして終える
    return withDaemon(config.socket, async (client) =>
      runAttachLoop({
        client,
        io,
        keys: config.keys,
        initial: await resolvePane(client, target!, io.homeDir),
        isReadonly: args.flag('readonly'),
        replay: args.flag('no-replay') ? 'none' : 'snapshot',
      }),
    );
  },
};
