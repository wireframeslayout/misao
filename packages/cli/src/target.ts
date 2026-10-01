import type { PaneInfo } from '@misao/protocol';
import { CliError } from './errors.js';
import type { ErrorCandidate } from './errors.js';
import { MIN_ID_FRAGMENT_LENGTH, displayName, formatWindowId, isWindowNumberQuery, stripWindowPrefix } from './view/pane-view.js';

/** help に載せる対象指定の書き方。数字だけの指定は窓番号として扱う。 */
export const TARGET_HELP = [
  '対象 <target> の書き方:',
  '  806 / W-806     窓番号 (数字だけの指定は窓番号として扱う)',
  '  p_01M3…, 7Q     pane ID、または ID の前方・後方一致 (2 文字以上)。数字だけの ID は p_ を付けて書く (p_12)',
  '  task:N          task ラベル',
  '  agent:ID        agent ラベル',
  '  名前の一部      name ラベルか窓名の部分一致',
];

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
  const bareWindow = stripWindowPrefix(query).toUpperCase();
  stageList.push((p) => {
    const id = p.labels.windowId;
    return id !== undefined && id !== '' && stripWindowPrefix(id).toUpperCase() === bareWindow;
  });
  // 窓番号の形 (806 / W-806) は窓番号の段だけで決める。閉じた窓の番号が ID や名前の部分一致で別のペインに当たらないようにする。
  if (isWindowNumberQuery(query)) return stageList;
  if (bareQuery.length >= MIN_ID_FRAGMENT_LENGTH) {
    stageList.push((p) => {
      const bare = p.paneId.slice(2).toUpperCase();
      return bare.startsWith(bareQuery) || bare.endsWith(bareQuery);
    });
  }
  stageList.push((p) => {
    const names = [p.labels.name, p.window.name].filter((n): n is string => n !== undefined && n !== '');
    return names.some((n) => n.toLowerCase().includes(lower));
  });
  return stageList;
}

/**
 * 対象指定 (pane ID / task:N / agent:ID / 窓番号 / ID の前方・後方一致 / 名前の部分一致) から pane を 1 つ決める。
 * 窓番号の形のクエリは窓番号の段だけで決める。0 件は not_found、同じ段階で複数なら ambiguous (候補付き)。
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
  if (isWindowNumberQuery(query)) {
    throw new CliError(
      'not_found',
      `${formatWindowId(stripWindowPrefix(query))} は見つかりません。misao ls で確認してください（数字だけの pane ID は p_ を付けて指定します）`,
    );
  }
  throw new CliError('not_found', `"${query}" に当てはまるペインがありません`);
}
