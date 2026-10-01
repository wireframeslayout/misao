import { withDaemon } from '../connect.js';
import { writeJson } from '../output.js';
import type { Command } from './command.js';

export const schemaCommand: Command = {
  name: 'schema',
  summary: 'プロトコルの JSON Schema (server.schema) をそのまま出力する',
  usage: 'misao schema',
  options: {},
  async run({ io, config }) {
    const schema = await withDaemon(config.socket, (client) => client.request('server.schema', {}));
    writeJson(io, schema);
    return 0;
  },
};
