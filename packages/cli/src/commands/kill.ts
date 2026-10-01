import { withDaemon } from '../connect.js';
import { confirm } from '../confirm.js';
import { CliError } from '../errors.js';
import { writeJson, writeLine } from '../output.js';
import { resolveTarget } from '../target.js';
import { displayName } from '../view/pane-view.js';
import type { Command } from './command.js';
import { expectPositionals } from './shared.js';

const USAGE = 'misao kill <target> [--window | --workspace] [--force] [--json]';

export const killCommand: Command = {
  name: 'kill',
  summary: 'ペインを閉じる（--window / --workspace で所属ごと閉じる）',
  usage: USAGE,
  options: { force: { type: 'boolean' }, window: { type: 'boolean' }, workspace: { type: 'boolean' } },
  async run({ args, io, isJson, config }) {
    const [target] = expectPositionals(args, 1, 1, USAGE);
    const scope = args.flag('window') ? 'window' : args.flag('workspace') ? 'workspace' : 'pane';
    if (args.flag('window') && args.flag('workspace')) {
      throw new CliError('usage', '--window と --workspace は同時に指定できません');
    }
    const isForced = args.flag('force');
    if (!isForced && !io.stdin.isTTY) {
      throw new CliError('usage', '確認できないため閉じません。端末から実行するか --force を付けてください');
    }
    return withDaemon(config.socket, async (client) => {
      const panes = await client.request('pane.list', {});
      const pane = resolveTarget(target!, panes, io.homeDir);
      const inWindow = panes.filter((p) => p.window.id === pane.window.id);
      const inWorkspace = panes.filter((p) => p.workspace === pane.workspace);
      const label =
        scope === 'pane'
          ? displayName(pane, io.homeDir)
          : scope === 'window'
            ? `窓「${pane.window.name}」（ペイン ${inWindow.length} 個）`
            : `workspace「${pane.workspace}」（ペイン ${inWorkspace.length} 個）`;
      if (!isForced && !(await confirm(io, `[misao] ${label} を閉じます。よろしいですか [y/N] `))) {
        throw new CliError('aborted', '中止しました');
      }
      if (scope === 'pane') await client.request('pane.close', { paneId: pane.paneId });
      else if (scope === 'window') await client.request('window.close', { windowId: pane.window.id });
      else await client.request('workspace.close', { name: pane.workspace });
      const paneIds = (scope === 'pane' ? [pane] : scope === 'window' ? inWindow : inWorkspace).map((p) => p.paneId);
      if (isJson) writeJson(io, { closed: scope, paneIds });
      else writeLine(io.stdout, `[misao] ${label} を閉じました`);
      return 0;
    });
  },
};
