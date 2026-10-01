import * as path from 'node:path';
import type { PaneInfo } from '@misao/protocol';

/** 一覧での並び順と表示に使う状態。processState が stopped のものは stopped 扱い。 */
export type PaneStateKey = 'blocked' | 'working' | 'idle' | 'exited' | 'stopped' | 'unknown';

const STATE_ORDER: readonly PaneStateKey[] = ['blocked', 'working', 'idle', 'exited', 'stopped', 'unknown'];

const STATE_SYMBOLS: Record<PaneStateKey, string> = {
  blocked: '●',
  working: '◐',
  idle: '○',
  exited: '✕',
  stopped: '■',
  unknown: '?',
};

const NO_VALUE = '—';
const ID_PREFIX_LENGTH = 4;
/** ID の前方・後方一致に使う最短の長さ。短縮 ID の末尾もこの長さ以上にする (照合と表示で共有)。 */
export const MIN_ID_FRAGMENT_LENGTH = 2;
const WINDOW_ID_PREFIX = /^w-/i;
const WINDOW_NUMBER_QUERY = /^(w-)?\d+$/i;

export function paneStateKey(pane: PaneInfo): PaneStateKey {
  return pane.processState === 'stopped' ? 'stopped' : pane.agentState;
}

export function stateLabel(pane: PaneInfo): string {
  const key = paneStateKey(pane);
  return `${STATE_SYMBOLS[key]} ${key}`;
}

/** blocked → working → idle → exited → stopped → unknown。同状態の中は lastOutputAt の新しい順。 */
export function sortPanes(panes: readonly PaneInfo[]): PaneInfo[] {
  const time = (p: PaneInfo): number => (p.lastOutputAt === null ? -Infinity : Date.parse(p.lastOutputAt));
  return [...panes].sort((a, b) => {
    const byState = STATE_ORDER.indexOf(paneStateKey(a)) - STATE_ORDER.indexOf(paneStateKey(b));
    if (byState !== 0) return byState;
    const ta = time(a);
    const tb = time(b);
    if (ta === tb) return a.paneId.localeCompare(b.paneId);
    return ta > tb ? -1 : 1;
  });
}

/** hub に登録された窓か (windowId ラベルがある)。 */
export function isRegistered(pane: PaneInfo): boolean {
  const id = pane.labels.windowId;
  return id !== undefined && id !== '';
}

/** hub 側の窓番号 806 を W-806 と表示する。すでに W- が付いていればそのまま。 */
export function formatWindowId(windowId: string): string {
  return `W-${stripWindowPrefix(windowId)}`;
}

/** 窓番号の先頭の w- / W- を除く。 */
export function stripWindowPrefix(windowId: string): string {
  return windowId.replace(WINDOW_ID_PREFIX, '');
}

/** 窓番号の形 (806 / W-806) の対象指定か。 */
export function isWindowNumberQuery(query: string): boolean {
  return WINDOW_NUMBER_QUERY.test(query);
}

/** ホームディレクトリ配下を ~ に短縮する。 */
export function tildify(cwd: string, homeDir: string): string {
  if (cwd === homeDir) return '~';
  return cwd.startsWith(`${homeDir}/`) ? `~${cwd.slice(homeDir.length)}` : cwd;
}

/** 前面コマンド。取れていなければ起動コマンドの basename。 */
export function foregroundCommand(pane: PaneInfo): string {
  return pane.fgCommand !== undefined && pane.fgCommand !== '' ? pane.fgCommand : path.basename(pane.cmd[0] ?? '');
}

/**
 * NAME 列の表示。
 * 登録済み: `W-806 · 表示名` / `W-806`。未登録: `(未登録) <name>`、name も無ければ `(未登録) <前面コマンド> · <cwd>`。
 */
export function displayName(pane: PaneInfo, homeDir: string): string {
  const { windowId, name } = pane.labels;
  const hasName = name !== undefined && name !== '';
  if (isRegistered(pane)) {
    const id = formatWindowId(windowId!);
    return hasName ? `${id} · ${name}` : id;
  }
  return hasName ? `(未登録) ${name}` : `(未登録) ${foregroundCommand(pane)} · ${tildify(pane.cwd, homeDir)}`;
}

/** task ラベルの値から先頭の # を除く。 */
export function taskNumber(pane: PaneInfo): string | undefined {
  const task = pane.labels.task;
  return task === undefined || task === '' ? undefined : task.replace(/^#/, '');
}

export function taskLabel(pane: PaneInfo): string {
  const task = taskNumber(pane);
  return task === undefined ? NO_VALUE : `#${task}`;
}

/** lastOutputAt からの相対時間 (12s / 4m / 2h)。無ければ —。 */
export function relativeTime(iso: string | null, now: number): string {
  if (iso === null) return NO_VALUE;
  const seconds = Math.max(0, Math.round((now - Date.parse(iso)) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h`;
}

/**
 * `p_01M3…7Q` 形式 (先頭 4 文字 + … + 一意になる最短の末尾、最小 2)。
 * 末尾は単独で対象指定 (`misao attach 7Q`) に使うので、resolveTarget で他の pane に当たらない長さまで伸ばす:
 * 他の ID の先頭・末尾 (大文字小文字を区別しない) と一致せず、窓番号の段に吸われる形 (数字だけ、
 * またはいずれかの pane の windowId と等しい) でもない長さ。
 */
export function shortPaneIds(panes: readonly PaneInfo[]): Map<string, string> {
  const result = new Map<string, string>();
  const bodies = panes.map((p) => p.paneId.slice(2).toUpperCase());
  const windowIds = new Set(
    panes.map((p) => p.labels.windowId).filter((w): w is string => w !== undefined && w !== '').map((w) => stripWindowPrefix(w).toUpperCase()),
  );
  panes.forEach((pane, index) => {
    const body = pane.paneId.slice(2);
    const others = bodies.filter((_, i) => i !== index);
    const isUnusable = (suffix: string): boolean =>
      isWindowNumberQuery(suffix) || windowIds.has(suffix) || others.some((o) => o.startsWith(suffix) || o.endsWith(suffix));
    let length = MIN_ID_FRAGMENT_LENGTH;
    while (length < body.length && isUnusable(body.slice(-length).toUpperCase())) length++;
    result.set(pane.paneId, `p_${body.slice(0, ID_PREFIX_LENGTH)}…${body.slice(-length)}`);
  });
  return result;
}

/** shortPaneIds の末尾部分 (`p_01M3…R8` の R8)。attach / kill の対象指定にそのまま使える。 */
export function shortSuffix(shortId: string): string {
  return shortId.slice(shortId.indexOf('…') + 1);
}

/** 「W-806 から抜けました」のような文中の呼び名。登録済みなら窓番号だけ、未登録なら NAME 列と同じ。 */
export function shortDisplayName(pane: PaneInfo, homeDir: string): string {
  return isRegistered(pane) ? formatWindowId(pane.labels.windowId!) : displayName(pane, homeDir);
}
