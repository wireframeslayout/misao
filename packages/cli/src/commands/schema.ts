import { withDaemon } from '../connect.js';
import { writeJson } from '../output.js';
import type { Command } from './command.js';
import { expectPositionals } from './shared.js';

const USAGE = 'misao schema';

export const schemaCommand: Command = {
  name: 'schema',
  summary: 'プロトコルの JSON Schema (server.schema) をそのまま出力する',
  usage: USAGE,
  options: {},
  async run({ args, io, config }) {
    expectPositionals(args, 0, 0, USAGE);
    const schema = await withDaemon(config.socket, (client) => client.request('server.schema', {}));
    writeJson(io, schema);
    return 0;
  },
};
