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
  /** 保存する。失敗は例外で伝播させる。 */
  persist(): void;
}

type LayoutHandlers = { [M in LayoutMethod]: (params: ParsedParams<M>) => unknown };

/** 配下の pane を全部閉じる。閉じている間に開かれた pane も残さない。 */
async function closePanesIn(host: LayoutHost, windowIds: ReadonlySet<string>): Promise<void> {
  for (let ids = host.paneIdsIn(windowIds); ids.length > 0; ids = host.paneIdsIn(windowIds)) {
    await Promise.all(ids.map((id) => host.closePane(id)));
  }
}

/** layout を変更して保存する。保存に失敗したら layout を元に戻して例外を伝播させる。 */
function commit<T>(host: LayoutHost, mutate: () => T): T {
  const before = host.layout.toPersisted();
  try {
    const result = mutate();
    host.persist();
    return result;
  } catch (e) {
    host.layout.restore(before);
    throw e;
  }
}

/**
 * 配下の pane を閉じる。閉じている間は layout が closing として配下への作成・pane.open を拒否する。
 * pane を閉じるのに失敗したら closing を取り消す。
 */
async function closeWithPanes(host: LayoutHost, windowIds: string[], cancel: () => void): Promise<void> {
  try {
    await closePanesIn(host, new Set(windowIds));
  } catch (e) {
    cancel();
    throw e;
  }
}

/**
 * 保存してからイベントを出す (イベントを観測した側が、保存済みの状態を前提にできる)。
 * 作成・rename は保存に失敗したら戻す。close は pane のプロセスを既に終了させているので戻さず、
 * メモリは実態 (閉じ済み) のままにして、保存に失敗してもイベントを出してから例外を投げ直す。
 */
export function createLayoutHandlers(host: LayoutHost): LayoutHandlers {
  const { layout, events } = host;
  return {
    'workspace.list': () => layout.list(),
    'workspace.create': (p) => {
      const info = commit(host, () => layout.createWorkspace(p.name));
      events.emit('workspace.created', { name: p.name });
      return info;
    },
    'workspace.rename': (p) => {
      commit(host, () => layout.renameWorkspace(p.name, p.newName));
      events.emit('workspace.renamed', { name: p.name, newName: p.newName });
      return { ok: true };
    },
    'workspace.close': async (p) => {
      const windowIds = layout.beginCloseWorkspace(p.name);
      await closeWithPanes(host, windowIds, () => layout.cancelCloseWorkspace(p.name));
      layout.closeWorkspace(p.name);
      try {
        host.persist();
      } finally {
        for (const windowId of windowIds) events.emit('window.closed', { windowId });
        events.emit('workspace.closed', { name: p.name });
      }
      return { ok: true };
    },
    'window.create': (p) => {
      const info = commit(host, () => layout.createWindow(p.workspace, p.name));
      events.emit('window.created', { windowId: info.windowId, workspace: p.workspace, name: p.name });
      return info;
    },
    'window.rename': (p) => {
      commit(host, () => layout.renameWindow(p.windowId, p.name));
      events.emit('window.renamed', { windowId: p.windowId, name: p.name });
      return { ok: true };
    },
    'window.close': async (p) => {
      layout.beginCloseWindow(p.windowId);
      await closeWithPanes(host, [p.windowId], () => layout.cancelCloseWindow(p.windowId));
      layout.closeWindow(p.windowId);
      try {
        host.persist();
      } finally {
        events.emit('window.closed', { windowId: p.windowId });
      }
      return { ok: true };
    },
  };
}
