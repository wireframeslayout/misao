import type { EventLog } from './event-log.js';
import type { Layout } from './layout.js';
import type { ParsedParams } from './params.js';

export type LayoutMethod =
  | 'workspace.list'
  | 'workspace.create'
  | 'workspace.close'
  | 'workspace.rename'
  | 'window.create'
  | 'window.close'
  | 'window.rename';

/** Daemon から受け取る依存。layout-handlers は Daemon 本体を知らない。 */
export interface LayoutHost {
  layout: Layout;
  events: EventLog;
  /** pane.close と同じ手順で閉じる (pane.closed まで出す)。 */
  closePane(paneId: string): Promise<void>;
  /** 指定 window に属する pane の ID。 */
  paneIdsIn(windowIds: ReadonlySet<string>): string[];
  /** 保存する。失敗は例外で伝播させる (メモリは戻さない)。 */
  persist(): void;
  /** mutate を実行して保存する。保存に失敗したら layout を元に戻して例外を伝播させる。 */
  commit<T>(mutate: () => T): T;
}

type LayoutHandlers = { [M in LayoutMethod]: (params: ParsedParams<M>) => unknown };

/** 配下の pane を全部閉じる。閉じている間に開かれた pane も残さない。 */
async function closePanesIn(host: LayoutHost, windowIds: ReadonlySet<string>): Promise<void> {
  for (let ids = host.paneIdsIn(windowIds); ids.length > 0; ids = host.paneIdsIn(windowIds)) {
    await Promise.all(ids.map((id) => host.closePane(id)));
  }
}

/**
 * 保存してからイベントを出す (イベントを観測した側が、保存済みの状態を前提にできる)。
 * 作成・rename は保存に失敗したら戻す。close は pane のプロセスを既に終了させているので戻さず、
 * メモリはプロセスの実態 (閉じ済み) に合わせたまま例外を伝播させる。
 */
export function createLayoutHandlers(host: LayoutHost): LayoutHandlers {
  const { layout, events } = host;
  return {
    'workspace.list': () => layout.list(),
    'workspace.create': (p) => {
      const info = host.commit(() => layout.createWorkspace(p.name));
      events.emit('workspace.created', { name: p.name });
      return info;
    },
    'workspace.rename': (p) => {
      host.commit(() => layout.renameWorkspace(p.name, p.newName));
      events.emit('workspace.renamed', { name: p.name, newName: p.newName });
      return { ok: true };
    },
    'workspace.close': async (p) => {
      // await 中の rename / close に備え、対象は先に確定し、後は ID で閉じる (既に無ければ成功扱い)。
      const knownIds = layout.requireWindowIds(p.name);
      await closePanesIn(host, new Set(knownIds));
      const exists = layout.hasWorkspace(p.name);
      const removed = exists ? layout.closeWorkspace(p.name) : layout.closeWindows(knownIds);
      host.persist();
      for (const windowId of removed) events.emit('window.closed', { windowId });
      if (exists) events.emit('workspace.closed', { name: p.name });
      return { ok: true };
    },
    'window.create': (p) => {
      const info = host.commit(() => layout.createWindow(p.workspace, p.name));
      events.emit('window.created', { windowId: info.windowId, workspace: p.workspace, name: p.name });
      return info;
    },
    'window.rename': (p) => {
      host.commit(() => layout.renameWindow(p.windowId, p.name));
      events.emit('window.renamed', { windowId: p.windowId, name: p.name });
      return { ok: true };
    },
    'window.close': async (p) => {
      layout.windowRef(p.windowId); // 無ければ WindowNotFound (pane を閉じる前に弾く)
      await closePanesIn(host, new Set([p.windowId]));
      const removed = layout.closeWindows([p.windowId]);
      host.persist();
      for (const windowId of removed) events.emit('window.closed', { windowId });
      return { ok: true };
    },
  };
}
