import { ErrorCode } from '@misao/protocol';
import type { PaneInfo, WindowInfo, WorkspaceInfo } from '@misao/protocol';
import type { WindowDef, WorkspaceDef } from './model.js';
import { RpcFailure } from './rpc-error.js';
import { newWindowId } from './ulid.js';

const DEFAULT_WORKSPACE = 'default';
const DEFAULT_WINDOW = 'default';

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
  private workspaces: readonly WorkspaceDef[] = [];
  /** 閉じている最中の workspace 名と window ID。配下への作成・pane.open を拒否する (close 待機中の競合を防ぐ)。 */
  private readonly closingWorkspaces = new Set<string>();
  private readonly closingWindows = new Set<string>();

  list(): WorkspaceInfo[] {
    return this.workspaces.map(toInfo);
  }

  createWorkspace(name: string): WorkspaceInfo {
    if (this.find(name)) throw new RpcFailure(ErrorCode.AlreadyExists, `workspace already exists: ${name}`);
    const created: WorkspaceDef = { name, windows: [] };
    this.workspaces = [...this.workspaces, created];
    return toInfo(created);
  }

  renameWorkspace(name: string, newName: string): void {
    const target = this.require(name);
    this.assertWorkspaceOpen(name); // 閉じている最中は名前を変えない (同名の再作成で別物を消さないため)
    if (newName !== name && this.find(newName)) {
      throw new RpcFailure(ErrorCode.AlreadyExists, `workspace already exists: ${newName}`);
    }
    this.workspaces = this.workspaces.map((w) => (w === target ? { ...w, name: newName } : w));
  }

  /** close の開始。以後、この workspace と配下の window は closing として扱う。配下の windowId 群を返す。 */
  beginCloseWorkspace(name: string): string[] {
    const target = this.require(name);
    this.assertWorkspaceOpen(name);
    const ids = target.windows.map((w) => w.id);
    if (ids.some((id) => this.closingWindows.has(id))) {
      throw new RpcFailure(ErrorCode.WorkspaceNotFound, `workspace has a closing window: ${name}`);
    }
    this.closingWorkspaces.add(name);
    for (const id of ids) this.closingWindows.add(id);
    return ids;
  }

  /** close を取りやめる (pane の close に失敗したとき)。 */
  cancelCloseWorkspace(name: string): void {
    this.closingWorkspaces.delete(name);
    for (const id of this.windowIds(name)) this.closingWindows.delete(id);
  }

  /** workspace を削除する (closing の印も外す)。削除した windowId 群を返す。 */
  closeWorkspace(name: string): string[] {
    const target = this.require(name);
    this.workspaces = this.workspaces.filter((w) => w !== target);
    this.closingWorkspaces.delete(name);
    const ids = target.windows.map((w) => w.id);
    for (const id of ids) this.closingWindows.delete(id);
    return ids;
  }

  createWindow(workspace: string, name: string): WindowInfo {
    const target = this.require(workspace);
    this.assertWorkspaceOpen(workspace);
    const window = { id: newWindowId(), name };
    this.workspaces = this.workspaces.map((w) => (w === target ? { ...w, windows: [...w.windows, window] } : w));
    return { windowId: window.id, name, workspace };
  }

  renameWindow(windowId: string, name: string): void {
    this.requireWindow(windowId);
    this.updateWindows((w) => w.windows.map((win) => (win.id === windowId ? { ...win, name } : win)));
  }

  beginCloseWindow(windowId: string): void {
    this.requireWindow(windowId);
    this.assertWindowOpen(windowId);
    this.closingWindows.add(windowId);
  }

  cancelCloseWindow(windowId: string): void {
    this.closingWindows.delete(windowId);
  }

  closeWindow(windowId: string): void {
    this.requireWindow(windowId);
    this.updateWindows((w) => w.windows.filter((win) => win.id !== windowId));
    this.closingWindows.delete(windowId);
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
    if (windowId !== undefined) {
      this.assertWindowOpen(windowId);
      return { ...this.requireWindow(windowId), createdWorkspace: false, createdWindow: false };
    }
    const createdWorkspace = !this.find(DEFAULT_WORKSPACE);
    if (createdWorkspace) this.createWorkspace(DEFAULT_WORKSPACE);
    this.assertWorkspaceOpen(DEFAULT_WORKSPACE);
    const isOpenDefault = (w: WindowDef): boolean => w.name === DEFAULT_WINDOW && !this.closingWindows.has(w.id);
    const existing = this.require(DEFAULT_WORKSPACE).windows.find(isOpenDefault);
    if (existing) return { workspace: DEFAULT_WORKSPACE, window: { id: existing.id, name: existing.name }, createdWorkspace, createdWindow: false };
    const created = this.createWindow(DEFAULT_WORKSPACE, DEFAULT_WINDOW);
    return {
      workspace: DEFAULT_WORKSPACE,
      window: { id: created.windowId, name: created.name },
      createdWorkspace,
      createdWindow: true,
    };
  }

  restore(persisted: readonly WorkspaceDef[]): void {
    this.workspaces = copyWorkspaces(persisted);
  }

  toPersisted(): WorkspaceDef[] {
    return copyWorkspaces(this.workspaces);
  }

  private assertWorkspaceOpen(name: string): void {
    if (this.closingWorkspaces.has(name)) throw new RpcFailure(ErrorCode.WorkspaceNotFound, `workspace is closing: ${name}`);
  }

  private assertWindowOpen(windowId: string): void {
    if (this.closingWindows.has(windowId)) throw new RpcFailure(ErrorCode.WindowNotFound, `window is closing: ${windowId}`);
  }

  private find(name: string): WorkspaceDef | undefined {
    return this.workspaces.find((w) => w.name === name);
  }

  private require(name: string): WorkspaceDef {
    const found = this.find(name);
    if (!found) throw new RpcFailure(ErrorCode.WorkspaceNotFound, `workspace not found: ${name}`);
    return found;
  }

  private requireWindow(windowId: string): WindowLocation {
    for (const w of this.workspaces) {
      const window = w.windows.find((win) => win.id === windowId);
      if (window) return { workspace: w.name, window: { id: window.id, name: window.name } };
    }
    throw new RpcFailure(ErrorCode.WindowNotFound, `window not found: ${windowId}`);
  }

  private updateWindows(fn: (w: WorkspaceDef) => WorkspaceDef['windows']): void {
    this.workspaces = this.workspaces.map((w) => ({ ...w, windows: fn(w) }));
  }
}

function copyWorkspaces(src: readonly WorkspaceDef[]): WorkspaceDef[] {
  return src.map((w) => ({ name: w.name, windows: w.windows.map((win) => ({ ...win })) }));
}

function toInfo(w: WorkspaceDef): WorkspaceInfo {
  return { name: w.name, windows: w.windows.map((win) => ({ windowId: win.id, name: win.name, workspace: w.name })) };
}
