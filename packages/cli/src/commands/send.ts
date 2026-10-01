import { withDaemon } from '../connect.js';
import { CliError } from '../errors.js';
import { writeJson, writeLine } from '../output.js';
import { encodeKeys } from '../send-keys.js';
import type { CliInput } from '../cli-io.js';
import type { Command } from './command.js';
import { expectPositionals, resolvePane } from './shared.js';

const USAGE = 'misao send <target> [text] [--enter] [--keys Enter,Escape,C-c] [--stdin] [--json]（- で始まる text は -- の後ろに書く）';

async function readAll(input: CliInput): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of input) chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk);
  return Buffer.concat(chunks).toString('utf8');
}

export const sendCommand: Command = {
  name: 'send',
  summary: 'ペインへテキストやキーを送る',
  usage: USAGE,
  options: { enter: { type: 'boolean' }, keys: { type: 'string' }, stdin: { type: 'boolean' } },
  async run({ args, io, isJson, config }) {
    const [target, positionalText] = expectPositionals(args, 1, 2, USAGE);
    if (positionalText !== undefined && args.rest.length > 0) {
      throw new CliError('usage', `text と -- の後ろの text は同時に指定できません。使い方: ${USAGE}`);
    }
    const text = positionalText ?? (args.rest.length > 0 ? args.rest.join(' ') : undefined);
    const keys = args.string('keys');
    const isEnter = args.flag('enter');
    if (args.flag('stdin') && text !== undefined) throw new CliError('usage', '--stdin と text は同時に指定できません');
    if (text === undefined && keys === undefined && !isEnter && !args.flag('stdin')) {
      throw new CliError('usage', `送る内容がありません。使い方: ${USAGE}`);
    }
    const body = args.flag('stdin') ? await readAll(io.stdin) : (text ?? '');
    const data = Buffer.concat([
      Buffer.from(body, 'utf8'),
      keys === undefined ? Buffer.alloc(0) : encodeKeys(keys.split(',').filter((k) => k !== '')),
      isEnter ? encodeKeys(['Enter']) : Buffer.alloc(0),
    ]);
    const paneId = await withDaemon(config.socket, async (client) => {
      const pane = await resolvePane(client, target!, io.homeDir);
      await client.request('pane.write', { paneId: pane.paneId, dataB64: data.toString('base64'), source: 'terminal' });
      return pane.paneId;
    });
    if (isJson) writeJson(io, { ok: true, paneId, bytes: data.length });
    else writeLine(io.stdout, `[misao] ${paneId} へ ${data.length} バイト送りました`);
    return 0;
  },
};
