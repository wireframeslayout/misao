import type { PaneInfo } from '@misao/protocol';
import type { CliIo } from '../cli-io.js';
import { askLine } from '../prompt.js';
import { writeLine } from '../output.js';
import { displayName, foregroundCommand, paneStateKey, relativeTime, shortDisplayName, sortPanes, stateLabel, taskLabel } from '../view/pane-view.js';
import { renderTable } from '../view/table.js';

export type PickerChoice = { kind: 'pane'; pane: PaneInfo } | { kind: 'new' } | { kind: 'quit' };

/** attach できるペイン (終了済み・停止中を除く)。ls と同じ並び。 */
export function attachablePanes(panes: readonly PaneInfo[]): PaneInfo[] {
  return sortPanes(panes).filter((p) => {
    const key = paneStateKey(p);
    return key !== 'exited' && key !== 'stopped';
  });
}

/** 一覧で blocked だったペインが、入る時点では working になっていれば知らせる文言。 */
export function resumedNotice(listed: PaneInfo, now: PaneInfo, homeDir: string): string | undefined {
  if (listed.agentState !== 'blocked' || now.agentState !== 'working') return undefined;
  return `${shortDisplayName(now, homeDir)} はいま再開しました（blocked → working）`;
}

export interface PickerOptions {
  /** 一覧の前に出す行 (抜けたときの案内など)。 */
  header: readonly string[];
  /** n (新しいペイン) を受け付けるか。--readonly では false。 */
  canCreate: boolean;
}

/** 番号付きの一覧を出し、番号 / n / q を受け付ける。入力が閉じたら quit。 */
export async function runPicker(io: CliIo, panes: readonly PaneInfo[], options: PickerOptions): Promise<PickerChoice> {
  const choices = attachablePanes(panes);
  for (const line of options.header) writeLine(io.stdout, line);
  if (choices.length === 0) {
    writeLine(io.stdout, '[misao] 入れるペインがありません');
  } else {
    const rows = choices.map((p, i) => [
      String(i + 1),
      stateLabel(p),
      displayName(p, io.homeDir),
      taskLabel(p),
      foregroundCommand(p),
      relativeTime(p.lastOutputAt, io.now()),
    ]);
    for (const line of renderTable([['#', 'STATE', 'NAME', 'TASK', 'AGENT', 'LAST'], ...rows])) {
      writeLine(io.stdout, `  ${line}`);
    }
  }
  const guide = ['番号で入る', ...(options.canCreate ? ['n で新しいペイン'] : []), 'q で終了'].join(' · ');
  writeLine(io.stdout, guide);
  for (;;) {
    const answer = (await askLine(io, '> '))?.trim().toLowerCase();
    if (answer === undefined || answer === 'q') return { kind: 'quit' };
    if (answer === 'n' && options.canCreate) return { kind: 'new' };
    const chosen = /^\d+$/.test(answer) ? choices[Number(answer) - 1] : undefined;
    if (chosen !== undefined) return { kind: 'pane', pane: chosen };
    writeLine(io.stderr, `[misao] ${guide} のいずれかを入力してください`);
  }
}
