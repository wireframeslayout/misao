import type { PaneInfo } from '@misao/protocol';
import { CliError } from './errors.js';
import type { ErrorCandidate } from './errors.js';
import { displayName } from './view/pane-view.js';

const WINDOW_ID_PREFIX = /^w-/i;

type Matcher = (pane: PaneInfo) => boolean;

/** 段階ごとに照合し、最初に 1 件以上当たった段階で決める。 */
function stages(query: string): Matcher[] {
  const upper = query.toUpperCase();
  const bareQuery = upper.replace(/^P_/, '');
  const lower = query.toLowerCase();
  const labelMatch = (key: 'task' | 'agent', value: string): Matcher => (p) => {
    const label = p.labels[key];
    return label !== undefined && label.replace(/^#/, '') === value.replace(/^#/, '');
  };
  const stageList: Matcher[] = [(p) => p.paneId === query];
  if (query.startsWith('task:')) stageList.push(labelMatch('task', query.slice('task:'.length)));
  if (query.startsWith('agent:')) stageList.push(labelMatch('agent', query.slice('agent:'.length)));
  const bareWindow = query.replace(WINDOW_ID_PREFIX, '');
  stageList.push((p) => {
    const id = p.labels.windowId;
    return id !== undefined && id !== '' && id.replace(WINDOW_ID_PREFIX, '') === bareWindow;
  });
  stageList.push((p) => {
    const bare = p.paneId.slice(2).toUpperCase();
    return bareQuery !== '' && (bare.startsWith(bareQuery) || bare.endsWith(bareQuery));
  });
  stageList.push((p) => {
    const names = [p.labels.name, p.window.name].filter((n): n is string => n !== undefined && n !== '');
    return names.some((n) => n.toLowerCase().includes(lower));
  });
  return stageList;
}

/**
 * 対象指定 (pane ID / ID の前方・後方一致 / 窓番号 / task:N / agent:ID / 名前の部分一致) から pane を 1 つ決める。
 * 0 件は not_found、同じ段階で複数なら ambiguous (候補付き)。
 */
export function resolveTarget(query: string, panes: readonly PaneInfo[], homeDir: string): PaneInfo {
  if (query === '') throw new CliError('usage', '対象を指定してください');
  for (const matches of stages(query)) {
    const hits = panes.filter(matches);
    if (hits.length === 1) return hits[0]!;
    if (hits.length > 1) {
      const candidates: ErrorCandidate[] = hits.map((p) => ({ paneId: p.paneId, name: displayName(p, homeDir) }));
      throw new CliError('ambiguous', `"${query}" に当てはまるペインが複数あります。絞り込んでください`, candidates);
    }
  }
  throw new CliError('not_found', `"${query}" に当てはまるペインがありません`);
}
