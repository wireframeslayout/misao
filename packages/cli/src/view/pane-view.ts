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
const MIN_SUFFIX_LENGTH = 2;

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
  return /^w-/i.test(windowId) ? `W-${windowId.slice(2)}` : `W-${windowId}`;
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
 * 一意かどうかは、先頭 4 文字が同じ pane の中で末尾を比べて決める。
 */
export function shortPaneIds(paneIds: readonly string[]): Map<string, string> {
  const result = new Map<string, string>();
  for (const id of paneIds) {
    const body = id.slice(2);
    const head = body.slice(0, ID_PREFIX_LENGTH);
    const peers = paneIds.filter((other) => other !== id && other.slice(2, 2 + ID_PREFIX_LENGTH) === head);
    let length = MIN_SUFFIX_LENGTH;
    while (length < body.length - ID_PREFIX_LENGTH && peers.some((o) => o.endsWith(body.slice(-length)))) length++;
    result.set(id, `p_${head}…${body.slice(-length)}`);
  }
  return result;
}

/** shortPaneIds の末尾部分 (`p_01M3…R8` の R8)。attach / kill の対象指定にそのまま使える。 */
export function shortSuffix(shortId: string): string {
  return shortId.slice(shortId.indexOf('…') + 1);
}
