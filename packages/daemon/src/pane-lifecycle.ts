import { ErrorCode } from '@misao/protocol';
import type { Layout, ResolvedWindow } from './layout.js';
import type { EventLog } from './event-log.js';
import type { ParsedParams } from './params.js';
import { Pane } from './pane.js';
import type { PaneRecord } from './model.js';
import type { PaneRegistry } from './pane-registry.js';
import { RpcFailure } from './rpc-error.js';
import type { StatePersister } from './state-persister.js';

/** Daemon から受け取る依存。PaneLifecycle は Daemon 本体を知らない。 */
export interface PaneLifecycleHost {
  socketPath: string;
  layout: Layout;
  registry: PaneRegistry;
  events: EventLog;
  persister: StatePersister;
  /** 全接続から、この pane の attachment と行購読を外す。 */
  releasePane(paneId: string): void;
}

/** pane の open / close / set_label。保存に失敗したときの戻し方もここで決める。 */
export class PaneLifecycle {
  constructor(private readonly host: PaneLifecycleHost) {}

  /**
   * layout (既定の遅延作成を含む) → spawn → record 登録 → 保存、を 1 つの単位として扱う。
   * 保存まで失敗したら全て戻し、起動した子プロセスも止め、イベントは一切出さない。
   */
  async open(p: ParsedParams<'pane.open'>): Promise<{ paneId: string }> {
    if (p.preplace !== undefined) throw new RpcFailure(ErrorCode.NotImplemented, 'preplace is not implemented');
    const { layout, registry, persister } = this.host;
    const before = layout.toPersisted();
    const target = layout.resolveWindow(p.windowId);
    let pane: Pane | undefined;
    try {
      pane = this.spawn(p);
      // 保存するのは env のみ。ephemeralEnv は Pane の spawn で使い切り、ここには渡らない。
      registry.add(toRecord(p, pane, target.window.id), pane);
      persister.saveNow();
    } catch (e) {
      layout.restore(before);
      if (pane) {
        registry.remove(pane.id);
        await pane.close();
        pane.dispose();
      }
      throw e;
    }
    this.announceOpened(pane, p.labels ?? {}, target);
    return { paneId: pane.id };
  }

  /**
   * live は SIGHUP → 終了待ち、stopped は record 削除のみ。どちらも保存してから pane.closed を出す。
   * live はプロセスを戻せないので、保存に失敗してもメモリは実態 (閉じ済み) のままにして
   * pane.closed を出してから例外を投げ直す。stopped は保存に失敗したら record を戻す。
   */
  async close(paneId: string): Promise<void> {
    const { registry, persister, events } = this.host;
    const { record, live } = registry.getOrThrow(paneId);
    if (!live) {
      registry.remove(paneId);
      try {
        persister.saveNow();
      } catch (e) {
        registry.restoreStopped([record]);
        throw e;
      }
      events.emit('pane.closed', {}, paneId);
      return;
    }
    await live.close();
    if (registry.get(paneId)?.live !== live) return; // 並行した close が後始末済み
    this.host.releasePane(paneId); // client.detached は pane.closed より前に出す
    registry.remove(paneId);
    live.dispose();
    try {
      persister.saveNow();
    } finally {
      events.emit('pane.closed', {}, paneId);
    }
  }

  /** 保存に失敗したら record (labels) を元に戻す。 */
  setLabel(p: ParsedParams<'pane.set_label'>): { labels: Record<string, string> } {
    const { registry, persister, events } = this.host;
    const before = registry.getOrThrow(p.paneId).record;
    const labels = registry.setLabels(p.paneId, p.set, p.unset);
    try {
      persister.saveNow();
    } catch (e) {
      registry.replaceRecord(before);
      throw e;
    }
    events.emit('pane.label', { set: p.set ?? {}, unset: p.unset ?? [] }, p.paneId);
    return { labels };
  }

  private spawn(p: ParsedParams<'pane.open'>): Pane {
    try {
      return new Pane({
        cmd: p.cmd,
        cwd: p.cwd,
        env: p.env,
        ephemeralEnv: p.ephemeralEnv,
        cols: p.cols,
        rows: p.rows,
        socketPath: this.host.socketPath,
      });
    } catch (e) {
      throw new RpcFailure(ErrorCode.InvalidParams, `spawn failed: ${(e as Error).message}`);
    }
  }

  /** 保存済みになってから、既定の workspace / window の作成と pane.opened を出す。 */
  private announceOpened(pane: Pane, labels: Record<string, string>, target: ResolvedWindow): void {
    const { events } = this.host;
    const { workspace, window, createdWorkspace, createdWindow } = target;
    pane.on('title', (title: string) => events.emit('pane.title', { title }, pane.id));
    pane.on('exit', (r: { exitCode: number | null; signal: number | null }) =>
      events.emit('pane.exited', { ...r }, pane.id),
    );
    if (createdWorkspace) events.emit('workspace.created', { name: workspace });
    if (createdWindow) events.emit('window.created', { windowId: window.id, workspace, name: window.name });
    events.emit('pane.opened', { pid: pane.pid, cmd: pane.cmd, labels, workspace, windowId: window.id }, pane.id);
  }
}

function toRecord(p: ParsedParams<'pane.open'>, pane: Pane, windowId: string): PaneRecord {
  return {
    paneId: pane.id,
    windowId,
    cmd: p.cmd,
    cwd: pane.cwd,
    env: p.env ?? {},
    labels: p.labels ?? {},
    cols: pane.cols,
    rows: pane.rows,
  };
}
