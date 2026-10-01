import type { PaneInfo } from '@misao/protocol';
import { withDaemon } from '../connect.js';
import { CliError } from '../errors.js';
import { writeJson, writeLine } from '../output.js';
import {
  displayName,
  foregroundCommand,
  paneStateKey,
  relativeTime,
  shortPaneIds,
  sortPanes,
  stateLabel,
  taskLabel,
  taskNumber,
  tildify,
} from '../view/pane-view.js';
import { renderTable } from '../view/table.js';
import type { Command } from './command.js';
import { expectPositionals } from './shared.js';

const STATE_FILTERS = ['blocked', 'working', 'idle', 'exited'] as const;
const USAGE = 'misao ls [--state blocked|working|idle|exited] [--task N] [--workspace W] [--json]';
const HEADER = ['STATE', 'PANE', 'NAME', 'TASK', 'AGENT', 'CWD', 'LAST'];

function isStateFilter(value: string): value is (typeof STATE_FILTERS)[number] {
  return (STATE_FILTERS as readonly string[]).includes(value);
}

/** shortIds は絞り込み前の全ペインから作る (表示した末尾を対象指定に使っても曖昧にならないように)。 */
function renderRows(panes: readonly PaneInfo[], shortIds: ReadonlyMap<string, string>, homeDir: string, now: number): string[] {
  const rows = panes.map((p) => [
    stateLabel(p),
    shortIds.get(p.paneId)!,
    displayName(p, homeDir),
    taskLabel(p),
    foregroundCommand(p),
    tildify(p.cwd, homeDir),
    relativeTime(p.lastOutputAt, now),
  ]);
  return renderTable([HEADER, ...rows]);
}

export const lsCommand: Command = {
  name: 'ls',
  summary: 'ペインの一覧を表示する（blocked が先頭）',
  usage: USAGE,
  options: { state: { type: 'string' }, task: { type: 'string' }, workspace: { type: 'string' } },
  async run({ args, io, isJson, config }) {
    expectPositionals(args, 0, 0, USAGE);
    const state = args.string('state');
    if (state !== undefined && !isStateFilter(state)) {
      throw new CliError('usage', `--state は ${STATE_FILTERS.join(' / ')} のいずれかを指定してください: ${state}`);
    }
    const task = args.string('task')?.replace(/^#/, '');
    const workspace = args.string('workspace');
    const all = await withDaemon(config.socket, (client) => client.request('pane.list', {}));
    const panes = sortPanes(all).filter(
      (p) =>
        (state === undefined || paneStateKey(p) === state) &&
        (task === undefined || taskNumber(p) === task) &&
        (workspace === undefined || p.workspace === workspace),
    );
    if (isJson) {
      writeJson(io, panes);
    } else if (panes.length === 0) {
      writeLine(io.stdout, '[misao] 該当するペインはありません');
    } else {
      for (const line of renderRows(panes, shortPaneIds(all.map((p) => p.paneId)), io.homeDir, io.now())) writeLine(io.stdout, line);
    }
    return 0;
  },
};
