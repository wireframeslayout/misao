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

/** 保存してからイベントを出す (イベントを観測した側が、保存済みの状態を前提にできる)。 */
export function createLayoutHandlers(host: LayoutHost): LayoutHandlers {
  const { layout, events } = host;
  return {
    'workspace.list': () => layout.list(),
    'workspace.create': (p) => {
      const info = layout.createWorkspace(p.name);
      host.persist();
      events.emit('workspace.created', { name: p.name });
      return info;
    },
    'workspace.rename': (p) => {
      layout.renameWorkspace(p.name, p.newName);
      host.persist();
      events.emit('workspace.renamed', { name: p.name, newName: p.newName });
      return { ok: true };
    },
    'workspace.close': async (p) => {
      await closePanesIn(host, new Set(layout.windowIds(p.name)));
      const windowIds = layout.closeWorkspace(p.name);
      host.persist();
      for (const windowId of windowIds) events.emit('window.closed', { windowId });
      events.emit('workspace.closed', { name: p.name });
      return { ok: true };
    },
    'window.create': (p) => {
      const info = layout.createWindow(p.workspace, p.name);
      host.persist();
      events.emit('window.created', { windowId: info.windowId, workspace: p.workspace, name: p.name });
      return info;
    },
    'window.rename': (p) => {
      layout.renameWindow(p.windowId, p.name);
      host.persist();
      events.emit('window.renamed', { windowId: p.windowId, name: p.name });
      return { ok: true };
    },
    'window.close': async (p) => {
      layout.windowRef(p.windowId); // 無ければ WindowNotFound (pane を閉じる前に弾く)
      await closePanesIn(host, new Set([p.windowId]));
      layout.closeWindow(p.windowId);
      host.persist();
      events.emit('window.closed', { windowId: p.windowId });
      return { ok: true };
    },
  };
}
