import * as path from 'node:path';
import { runAttachLoop } from '../attach/loop.js';
import { requireTerminal } from '../attach/tty.js';
import { parseKeyValues } from '../args.js';
import { withDaemon } from '../connect.js';
import { CliError } from '../errors.js';
import { resolveWindowForOpen } from '../layout-target.js';
import { loginShell, openTerminalPane } from '../open-pane.js';
import { writeJson, writeLine } from '../output.js';
import { foregroundCommand, shortPaneIds, shortSuffix, tildify } from '../view/pane-view.js';
import type { Command } from './command.js';
import { expectPositionals } from './shared.js';

const USAGE =
  'misao new [--cwd DIR] [--label k=v ...] [--env K=V ...] [--workspace W] [--window NAME] [--attach | --json] [-- cmd ...]';

export const newCommand: Command = {
  name: 'new',
  summary: '新しいペインを作る（cmd 省略時はログインシェル）',
  usage: USAGE,
  options: {
    cwd: { type: 'string' },
    label: { type: 'string', multiple: true },
    env: { type: 'string', multiple: true },
    workspace: { type: 'string' },
    window: { type: 'string' },
    attach: { type: 'boolean' },
  },
  async run({ args, io, isJson, config }) {
    expectPositionals(args, 0, 0, USAGE);
    const isAttach = args.flag('attach');
    if (isAttach && isJson) throw new CliError('usage', '--attach と --json は同時に指定できません');
    if (isAttach) requireTerminal(io); // 端末でなければ、ペインを作る前に使い方の誤りとして終える
    const cmd = args.rest.length > 0 ? [...args.rest] : loginShell(io.shell);
    const labels = parseKeyValues(args.strings('label'), '--label');
    const env = parseKeyValues(args.strings('env'), '--env');
    const cwd = path.resolve(io.cwd, args.string('cwd') ?? '.');
    return withDaemon(config.socket, async (client) => {
      const windowId = await resolveWindowForOpen(client, {
        workspace: args.string('workspace'),
        window: args.string('window'),
      });
      const paneId = await openTerminalPane(client, { cmd, cwd, labels, env, windowId });
      const panes = await client.request('pane.list', {});
      const created = panes.find((p) => p.paneId === paneId);
      if (created === undefined) throw new CliError('runtime', `作成したペインが一覧にありません: ${paneId}`);
      if (isAttach) {
        return runAttachLoop({ client, io, keys: config.keys, initial: created, isReadonly: false, replay: 'snapshot' });
      }
      if (isJson) {
        writeJson(io, created);
        return 0;
      }
      const shortId = shortPaneIds(panes.map((p) => p.paneId)).get(paneId)!;
      writeLine(io.stdout, `[misao] ペイン ${shortId} を作成しました（${foregroundCommand(created)} · ${tildify(created.cwd, io.homeDir)}）`);
      writeLine(io.stdout, '       AZITO の Objects に「未登録」として表示されます。登録すると窓として扱えます');
      writeLine(io.stdout, `       入る: misao attach ${shortSuffix(shortId)}`);
      return 0;
    });
  },
};
