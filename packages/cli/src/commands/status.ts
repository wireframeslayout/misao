import { connectDaemon } from '../connect.js';
import { CliError } from '../errors.js';
import { writeJson, writeLine } from '../output.js';
import type { Command } from './command.js';

function formatUptime(totalSec: number): string {
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = Math.floor(totalSec % 60);
  return [h > 0 ? `${h}h` : '', h > 0 || m > 0 ? `${m}m` : '', `${s}s`].filter(Boolean).join(' ');
}

export const statusCommand: Command = {
  name: 'status',
  summary: 'デーモンの稼働状況を表示する',
  usage: 'misao status [--json]',
  options: {},
  async run({ io, isJson, config }) {
    const socket = config.socket;
    let client;
    try {
      client = await connectDaemon(socket);
    } catch (error) {
      if (!(error instanceof CliError) || error.kind !== 'daemon_unreachable') throw error;
      if (isJson) writeJson(io, { running: false, socket });
      else writeLine(io.stdout, `[misao] 停止中（${socket}）`);
      return 1;
    }
    try {
      const info = await client.request('server.info', {});
      if (isJson) {
        writeJson(io, { running: true, ...info, socket });
        return 0;
      }
      writeLine(io.stdout, '[misao] 稼働中');
      writeLine(io.stdout, `  プロトコル  ${info.protocolVersion}`);
      writeLine(io.stdout, `  pid         ${info.pid}`);
      writeLine(io.stdout, `  epoch       ${info.epoch}`);
      writeLine(io.stdout, `  稼働時間    ${formatUptime(info.uptimeSec)}`);
      writeLine(io.stdout, `  ペイン数    ${info.paneCount}`);
      writeLine(io.stdout, `  ソケット    ${socket}`);
      return 0;
    } finally {
      client.close();
    }
  },
};
