import { ErrorCode } from '@misao/protocol';
import type { PaneInfo, WindowInfo, WorkspaceInfo } from '@misao/protocol';
import type { PersistedWorkspace } from './persistence.js';
import { RpcFailure } from './rpc-error.js';
import { newWindowId } from './ulid.js';

export const DEFAULT_WORKSPACE = 'default';
export const DEFAULT_WINDOW = 'default';

type WindowRef = PaneInfo['window'];

export interface WindowLocation {
  workspace: string;
  window: WindowRef;
}

/** resolveWindow の結果。created* は、このとき新しく作ったもの (呼び出し側がイベントを出す)。 */
export interface ResolvedWindow extends WindowLocation {
  createdWorkspace: boolean;
  createdWindow: boolean;
}

/**
 * workspace → window の in-memory モデル (I/O なし)。
 * workspace 名は一意、window 名は重複を許す。更新は新しいオブジェクトで置き換える。
 */
export class Layout {
  private workspaces: readonly PersistedWorkspace[] = [];

  list(): WorkspaceInfo[] {
    return this.workspaces.map(toInfo);
  }

  createWorkspace(name: string): WorkspaceInfo {
    if (this.find(name)) throw new RpcFailure(ErrorCode.AlreadyExists, `workspace already exists: ${name}`);
    const created: PersistedWorkspace = { name, windows: [] };
    this.workspaces = [...this.workspaces, created];
    return toInfo(created);
  }

  renameWorkspace(name: string, newName: string): void {
    const target = this.require(name);
    if (newName !== name && this.find(newName)) {
      throw new RpcFailure(ErrorCode.AlreadyExists, `workspace already exists: ${newName}`);
    }
    this.workspaces = this.workspaces.map((w) => (w === target ? { ...w, name: newName } : w));
  }

  /** 削除した workspace に属していた windowId 群を返す。 */
  closeWorkspace(name: string): string[] {
    const target = this.require(name);
    this.workspaces = this.workspaces.filter((w) => w !== target);
    return target.windows.map((w) => w.id);
  }

  createWindow(workspace: string, name: string): WindowInfo {
    const target = this.require(workspace);
    const window = { id: newWindowId(), name };
    this.workspaces = this.workspaces.map((w) => (w === target ? { ...w, windows: [...w.windows, window] } : w));
    return { windowId: window.id, name, workspace };
  }

  renameWindow(windowId: string, name: string): void {
    this.requireWindow(windowId);
    this.updateWindows((w) => w.windows.map((win) => (win.id === windowId ? { ...win, name } : win)));
  }

  /** 存在する window だけを削除し、削除したものの ID を返す (既に無いものは無視)。 */
  closeWindows(windowIds: readonly string[]): string[] {
    const removed = windowIds.filter((id) => this.hasWindow(id));
    this.updateWindows((w) => w.windows.filter((win) => !removed.includes(win.id)));
    return removed;
  }

  hasWorkspace(name: string): boolean {
    return this.find(name) !== undefined;
  }

  hasWindow(windowId: string): boolean {
    return this.workspaces.some((w) => w.windows.some((win) => win.id === windowId));
  }

  /** workspace に属する windowId 群。存在しなければ WorkspaceNotFound。 */
  requireWindowIds(name: string): string[] {
    return this.require(name).windows.map((w) => w.id);
  }

  windowRef(windowId: string): WindowLocation {
    return this.requireWindow(windowId);
  }

  /** workspace に属する windowId 群。存在しない workspace は空。 */
  windowIds(workspace: string): string[] {
    return this.find(workspace)?.windows.map((w) => w.id) ?? [];
  }

  /** windowId を解決する。省略時は既定 workspace の既定 window (無ければ作る)。 */
  resolveWindow(windowId: string | undefined): ResolvedWindow {
    if (windowId !== undefined) return { ...this.requireWindow(windowId), createdWorkspace: false, createdWindow: false };
    const createdWorkspace = !this.find(DEFAULT_WORKSPACE);
    if (createdWorkspace) this.createWorkspace(DEFAULT_WORKSPACE);
    const existing = this.require(DEFAULT_WORKSPACE).windows.find((w) => w.name === DEFAULT_WINDOW);
    if (existing) return { workspace: DEFAULT_WORKSPACE, window: existing, createdWorkspace, createdWindow: false };
    const created = this.createWindow(DEFAULT_WORKSPACE, DEFAULT_WINDOW);
    return {
      workspace: DEFAULT_WORKSPACE,
      window: { id: created.windowId, name: created.name },
      createdWorkspace,
      createdWindow: true,
    };
  }

  restore(persisted: readonly PersistedWorkspace[]): void {
    this.workspaces = copyWorkspaces(persisted);
  }

  toPersisted(): PersistedWorkspace[] {
    return copyWorkspaces(this.workspaces);
  }

  private find(name: string): PersistedWorkspace | undefined {
    return this.workspaces.find((w) => w.name === name);
  }

  private require(name: string): PersistedWorkspace {
    const found = this.find(name);
    if (!found) throw new RpcFailure(ErrorCode.WorkspaceNotFound, `workspace not found: ${name}`);
    return found;
  }

  private requireWindow(windowId: string): WindowLocation {
    for (const w of this.workspaces) {
      const window = w.windows.find((win) => win.id === windowId);
      if (window) return { workspace: w.name, window };
    }
    throw new RpcFailure(ErrorCode.WindowNotFound, `window not found: ${windowId}`);
  }

  private updateWindows(fn: (w: PersistedWorkspace) => PersistedWorkspace['windows']): void {
    this.workspaces = this.workspaces.map((w) => ({ ...w, windows: fn(w) }));
  }
}

function copyWorkspaces(src: readonly PersistedWorkspace[]): PersistedWorkspace[] {
  return src.map((w) => ({ name: w.name, windows: w.windows.map((win) => ({ ...win })) }));
}

function toInfo(w: PersistedWorkspace): WorkspaceInfo {
  return { name: w.name, windows: w.windows.map((win) => ({ windowId: win.id, name: win.name, workspace: w.name })) };
}
