import { ErrorCode } from '@misao/protocol';
import type { PaneInfo, ProcessState } from '@misao/protocol';
import type { PersistedPane } from './persistence.js';
import type { Pane } from './pane.js';
import { RpcFailure } from './rpc-error.js';

/** pane のメタ (persistence.json に保存するもの)。ephemeralEnv は持たない。 */
export type PaneRecord = PersistedPane;

/** record は常にある。live は、このデーモンが起動した pane のみ (再起動後の stopped には無い)。 */
export interface PaneEntry {
  record: PaneRecord;
  live?: Pane;
}

/** pane のメタ (record) と実行体 (live Pane) の対応。親 (window / workspace) は知らない。 */
export class PaneRegistry {
  private readonly entries = new Map<string, PaneEntry>();

  get size(): number {
    return this.entries.size;
  }

  add(record: PaneRecord, live: Pane): void {
    this.entries.set(record.paneId, { record, live });
  }

  restoreStopped(records: readonly PaneRecord[]): void {
    for (const record of records) this.entries.set(record.paneId, { record });
  }

  get(paneId: string): PaneEntry | undefined {
    return this.entries.get(paneId);
  }

  remove(paneId: string): void {
    this.entries.delete(paneId);
  }

  livePanes(): Pane[] {
    return [...this.entries.values()].flatMap((e) => (e.live ? [e.live] : []));
  }

  /** set → unset の順に適用した新しい labels を返し、record を置き換える。 */
  setLabels(paneId: string, set: Record<string, string> | undefined, unset: readonly string[] | undefined): Record<string, string> {
    const entry = this.require(paneId);
    const labels = { ...entry.record.labels, ...set };
    for (const key of unset ?? []) delete labels[key];
    this.entries.set(paneId, { ...entry, record: { ...entry.record, labels } });
    return labels;
  }

  /** 全条件の AND。windowIds が undefined なら window では絞らない。 */
  filter(
    state: ProcessState | undefined,
    labels: Record<string, string> | undefined,
    windowIds: ReadonlySet<string> | undefined,
  ): PaneEntry[] {
    return [...this.entries.values()]
      .filter((e) => state === undefined || processState(e) === state)
      .filter((e) => labels === undefined || Object.entries(labels).every(([k, v]) => e.record.labels[k] === v))
      .filter((e) => windowIds === undefined || windowIds.has(e.record.windowId));
  }

  info(paneId: string, workspace: string, window: PaneInfo['window']): PaneInfo {
    const { record, live } = this.require(paneId);
    if (live) return live.info({ workspace, window, labels: record.labels });
    return {
      paneId,
      pid: null,
      cmd: record.cmd,
      cwd: record.cwd,
      workspace,
      window,
      labels: record.labels,
      processState: 'stopped',
      exitCode: null,
      signal: null,
      agentState: 'unknown',
      decidedBy: 'none',
      title: '',
      lastOutputAt: null,
      cols: record.cols,
      rows: record.rows,
      clients: [],
      sizeOwner: null,
    };
  }

  /** live の現在の cols / rows を反映した保存用の record 群。 */
  toPersisted(): PaneRecord[] {
    return [...this.entries.values()].map(({ record, live }) =>
      live ? { ...record, cols: live.cols, rows: live.rows } : record,
    );
  }

  private require(paneId: string): PaneEntry {
    const entry = this.entries.get(paneId);
    if (!entry) throw new RpcFailure(ErrorCode.PaneNotFound, `pane not found: ${paneId}`);
    return entry;
  }
}

function processState({ live }: PaneEntry): ProcessState {
  return live ? live.state : 'stopped';
}
