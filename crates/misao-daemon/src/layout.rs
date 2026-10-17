use std::collections::HashSet;

use misao_protocol::ErrorCode;
use serde::Serialize;

use crate::ids::new_window_id;
use crate::params::{Name, WindowId};
use crate::persistence::{PersistedWindow, PersistedWorkspace};
use crate::rpc_error::{RpcFailure, RpcResult};

const DEFAULT_WORKSPACE: &str = "default";
const DEFAULT_WINDOW: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowDef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDef {
    pub name: String,
    pub windows: Vec<WindowDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub window_id: String,
    pub name: String,
    pub workspace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub windows: Vec<WindowInfo>,
}

/// `pane.info` の `window`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLocation {
    pub workspace: String,
    pub window: WindowRef,
}

/// `resolve_window` の結果。`created_*` は、このとき新しく作ったもの（呼び出し側がイベントを出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWindow {
    pub location: WindowLocation,
    pub created_workspace: bool,
    pub created_window: bool,
}

/// workspace → window のモデル（I/O なし）。workspace 名は一意、window 名は重複を許す。
#[derive(Debug, Clone, Default)]
pub struct Layout {
    workspaces: Vec<WorkspaceDef>,
    /// 閉じている最中の workspace 名と window ID。配下への作成・pane.open を拒否する（close 待機中の競合を防ぐ）。
    closing_workspaces: HashSet<String>,
    closing_windows: HashSet<String>,
}

fn to_info(w: &WorkspaceDef) -> WorkspaceInfo {
    WorkspaceInfo {
        name: w.name.clone(),
        windows: w
            .windows
            .iter()
            .map(|win| WindowInfo {
                window_id: win.id.clone(),
                name: win.name.clone(),
                workspace: w.name.clone(),
            })
            .collect(),
    }
}

fn already_exists(name: &str) -> RpcFailure {
    RpcFailure::new(
        ErrorCode::ALREADY_EXISTS,
        format!("workspace already exists: {name}"),
    )
}

impl Layout {
    pub fn list(&self) -> Vec<WorkspaceInfo> {
        self.workspaces.iter().map(to_info).collect()
    }

    pub fn create_workspace(&mut self, name: &str) -> RpcResult<WorkspaceInfo> {
        if self.find(name).is_some() {
            return Err(already_exists(name));
        }
        let created = WorkspaceDef {
            name: name.to_string(),
            windows: Vec::new(),
        };
        let info = to_info(&created);
        self.workspaces.push(created);
        Ok(info)
    }

    pub fn rename_workspace(&mut self, name: &str, new_name: &str) -> RpcResult<()> {
        let index = self.require(name)?;
        self.assert_workspace_open(name)?; // 閉じている最中は名前を変えない（同名の再作成で別物を消さないため）
        if new_name != name && self.find(new_name).is_some() {
            return Err(already_exists(new_name));
        }
        self.workspaces[index].name = new_name.to_string();
        Ok(())
    }

    /// close の開始。以後、この workspace と配下の window は closing として扱う。配下の windowId 群を返す。
    pub fn begin_close_workspace(&mut self, name: &str) -> RpcResult<Vec<String>> {
        let index = self.require(name)?;
        self.assert_workspace_open(name)?;
        let ids: Vec<String> = self.workspaces[index]
            .windows
            .iter()
            .map(|w| w.id.clone())
            .collect();
        if ids.iter().any(|id| self.closing_windows.contains(id)) {
            return Err(RpcFailure::new(
                ErrorCode::WORKSPACE_NOT_FOUND,
                format!("workspace has a closing window: {name}"),
            ));
        }
        self.closing_workspaces.insert(name.to_string());
        self.closing_windows.extend(ids.iter().cloned());
        Ok(ids)
    }

    /// close を取りやめる（pane の close に失敗したとき）。
    pub fn cancel_close_workspace(&mut self, name: &str) {
        self.closing_workspaces.remove(name);
        for id in self.window_ids(name) {
            self.closing_windows.remove(&id);
        }
    }

    /// workspace を削除する（closing の印も外す）。削除した windowId 群を返す。
    pub fn close_workspace(&mut self, name: &str) -> RpcResult<Vec<String>> {
        let index = self.require(name)?;
        let removed = self.workspaces.remove(index);
        self.closing_workspaces.remove(name);
        let ids: Vec<String> = removed.windows.into_iter().map(|w| w.id).collect();
        for id in &ids {
            self.closing_windows.remove(id);
        }
        Ok(ids)
    }

    pub fn create_window(&mut self, workspace: &str, name: &str) -> RpcResult<WindowInfo> {
        let index = self.require(workspace)?;
        self.assert_workspace_open(workspace)?;
        let window = WindowDef {
            id: new_window_id(),
            name: name.to_string(),
        };
        let info = WindowInfo {
            window_id: window.id.clone(),
            name: name.to_string(),
            workspace: workspace.to_string(),
        };
        self.workspaces[index].windows.push(window);
        Ok(info)
    }

    pub fn rename_window(&mut self, window_id: &str, name: &str) -> RpcResult<()> {
        self.require_window(window_id)?;
        for window in self.workspaces.iter_mut().flat_map(|w| w.windows.iter_mut()) {
            if window.id == window_id {
                window.name = name.to_string();
            }
        }
        Ok(())
    }

    pub fn begin_close_window(&mut self, window_id: &str) -> RpcResult<()> {
        self.require_window(window_id)?;
        self.assert_window_open(window_id)?;
        self.closing_windows.insert(window_id.to_string());
        Ok(())
    }

    pub fn cancel_close_window(&mut self, window_id: &str) {
        self.closing_windows.remove(window_id);
    }

    pub fn close_window(&mut self, window_id: &str) -> RpcResult<()> {
        self.require_window(window_id)?;
        for w in &mut self.workspaces {
            w.windows.retain(|win| win.id != window_id);
        }
        self.closing_windows.remove(window_id);
        Ok(())
    }

    pub fn window_ref(&self, window_id: &str) -> RpcResult<WindowLocation> {
        self.require_window(window_id)
    }

    /// workspace に属する windowId 群。存在しない workspace は空。
    pub fn window_ids(&self, workspace: &str) -> Vec<String> {
        self.find(workspace)
            .map(|w| {
                self.workspaces[w]
                    .windows
                    .iter()
                    .map(|win| win.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// windowId を解決する。省略時は既定 workspace の既定 window（無ければ作る）。
    pub fn resolve_window(&mut self, window_id: Option<&str>) -> RpcResult<ResolvedWindow> {
        if let Some(id) = window_id {
            self.assert_window_open(id)?;
            return Ok(ResolvedWindow {
                location: self.require_window(id)?,
                created_workspace: false,
                created_window: false,
            });
        }
        let created_workspace = self.find(DEFAULT_WORKSPACE).is_none();
        if created_workspace {
            self.create_workspace(DEFAULT_WORKSPACE)?;
        }
        self.assert_workspace_open(DEFAULT_WORKSPACE)?;
        let index = self.require(DEFAULT_WORKSPACE)?;
        let existing = self.workspaces[index]
            .windows
            .iter()
            .find(|w| w.name == DEFAULT_WINDOW && !self.closing_windows.contains(&w.id));
        if let Some(window) = existing {
            return Ok(ResolvedWindow {
                location: WindowLocation {
                    workspace: DEFAULT_WORKSPACE.to_string(),
                    window: WindowRef {
                        id: window.id.clone(),
                        name: window.name.clone(),
                    },
                },
                created_workspace,
                created_window: false,
            });
        }
        let created = self.create_window(DEFAULT_WORKSPACE, DEFAULT_WINDOW)?;
        Ok(ResolvedWindow {
            location: WindowLocation {
                workspace: DEFAULT_WORKSPACE.to_string(),
                window: WindowRef {
                    id: created.window_id,
                    name: created.name,
                },
            },
            created_workspace,
            created_window: true,
        })
    }

    /// 失敗した変更の巻き戻し用。workspace の一覧だけを保存・復元する（closing の印は対象外）。
    pub fn snapshot(&self) -> Vec<WorkspaceDef> {
        self.workspaces.clone()
    }

    pub fn restore(&mut self, snapshot: Vec<WorkspaceDef>) {
        self.workspaces = snapshot;
    }

    /// 起動時に、保存した定義から復元する。
    pub fn restore_persisted(&mut self, persisted: Vec<PersistedWorkspace>) {
        self.workspaces = persisted
            .into_iter()
            .map(|w| WorkspaceDef {
                name: String::from(w.name),
                windows: w
                    .windows
                    .into_iter()
                    .map(|win| WindowDef {
                        id: String::from(win.id),
                        name: String::from(win.name),
                    })
                    .collect(),
            })
            .collect();
    }

    /// 保存用の定義。名前と ID は作成時に検証済みなので、形式の違反は内部エラー。
    pub fn to_persisted(&self) -> Result<Vec<PersistedWorkspace>, String> {
        self.workspaces
            .iter()
            .map(|w| {
                Ok(PersistedWorkspace {
                    name: Name::try_from(w.name.clone())?,
                    windows: w
                        .windows
                        .iter()
                        .map(|win| {
                            Ok(PersistedWindow {
                                id: WindowId::try_from(win.id.clone())?,
                                name: Name::try_from(win.name.clone())?,
                            })
                        })
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect()
    }

    fn assert_workspace_open(&self, name: &str) -> RpcResult<()> {
        if self.closing_workspaces.contains(name) {
            return Err(RpcFailure::new(
                ErrorCode::WORKSPACE_NOT_FOUND,
                format!("workspace is closing: {name}"),
            ));
        }
        Ok(())
    }

    fn assert_window_open(&self, window_id: &str) -> RpcResult<()> {
        if self.closing_windows.contains(window_id) {
            return Err(RpcFailure::new(
                ErrorCode::WINDOW_NOT_FOUND,
                format!("window is closing: {window_id}"),
            ));
        }
        Ok(())
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.workspaces.iter().position(|w| w.name == name)
    }

    fn require(&self, name: &str) -> RpcResult<usize> {
        self.find(name).ok_or_else(|| {
            RpcFailure::new(
                ErrorCode::WORKSPACE_NOT_FOUND,
                format!("workspace not found: {name}"),
            )
        })
    }

    fn require_window(&self, window_id: &str) -> RpcResult<WindowLocation> {
        for w in &self.workspaces {
            if let Some(window) = w.windows.iter().find(|win| win.id == window_id) {
                return Ok(WindowLocation {
                    workspace: w.name.clone(),
                    window: WindowRef {
                        id: window.id.clone(),
                        name: window.name.clone(),
                    },
                });
            }
        }
        Err(RpcFailure::new(
            ErrorCode::WINDOW_NOT_FOUND,
            format!("window not found: {window_id}"),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn code<T: std::fmt::Debug>(r: RpcResult<T>) -> i64 {
        r.unwrap_err().code
    }

    #[test]
    fn workspace_names_are_unique_but_window_names_are_not() {
        let mut l = Layout::default();
        l.create_workspace("a").unwrap();
        assert_eq!(code(l.create_workspace("a")), ErrorCode::ALREADY_EXISTS);
        let w1 = l.create_window("a", "w").unwrap();
        let w2 = l.create_window("a", "w").unwrap();
        assert_ne!(w1.window_id, w2.window_id);
        assert!(w1.window_id.starts_with("w_"));
    }

    #[test]
    fn missing_targets_use_the_dedicated_codes() {
        let mut l = Layout::default();
        assert_eq!(code(l.create_window("none", "w")), ErrorCode::WORKSPACE_NOT_FOUND);
        assert_eq!(
            code(l.rename_workspace("none", "x")),
            ErrorCode::WORKSPACE_NOT_FOUND
        );
        assert_eq!(code(l.rename_window("w_x", "x")), ErrorCode::WINDOW_NOT_FOUND);
        assert_eq!(code(l.close_workspace("none")), ErrorCode::WORKSPACE_NOT_FOUND);
    }

    #[test]
    fn rename_workspace_rejects_a_taken_name_but_allows_the_same_name() {
        let mut l = Layout::default();
        l.create_workspace("a").unwrap();
        l.create_workspace("b").unwrap();
        assert_eq!(code(l.rename_workspace("a", "b")), ErrorCode::ALREADY_EXISTS);
        l.rename_workspace("a", "a").unwrap();
        l.rename_workspace("a", "c").unwrap();
        let names: Vec<_> = l.list().into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["c", "b"]);
    }

    #[test]
    fn resolve_creates_the_default_workspace_and_window_once() {
        let mut l = Layout::default();
        let first = l.resolve_window(None).unwrap();
        assert!(first.created_workspace && first.created_window);
        assert_eq!(first.location.workspace, "default");
        let second = l.resolve_window(None).unwrap();
        assert!(!second.created_workspace && !second.created_window);
        assert_eq!(first.location, second.location);
    }

    #[test]
    fn resolve_by_id_requires_an_open_window() {
        let mut l = Layout::default();
        let id = l.resolve_window(None).unwrap().location.window.id;
        assert_eq!(l.resolve_window(Some(&id)).unwrap().location.window.id, id);
        l.begin_close_window(&id).unwrap();
        assert_eq!(code(l.resolve_window(Some(&id))), ErrorCode::WINDOW_NOT_FOUND);
        l.cancel_close_window(&id);
        assert!(l.resolve_window(Some(&id)).is_ok());
    }

    #[test]
    fn closing_a_workspace_blocks_new_windows_until_cancelled() {
        let mut l = Layout::default();
        l.create_workspace("a").unwrap();
        let w = l.create_window("a", "w").unwrap();
        let ids = l.begin_close_workspace("a").unwrap();
        assert_eq!(ids, std::slice::from_ref(&w.window_id));
        assert_eq!(code(l.create_window("a", "x")), ErrorCode::WORKSPACE_NOT_FOUND);
        assert_eq!(code(l.begin_close_workspace("a")), ErrorCode::WORKSPACE_NOT_FOUND);
        l.cancel_close_workspace("a");
        l.create_window("a", "x").unwrap();
    }

    #[test]
    fn close_workspace_removes_its_windows() {
        let mut l = Layout::default();
        l.create_workspace("a").unwrap();
        let w = l.create_window("a", "w").unwrap();
        assert_eq!(
            l.close_workspace("a").unwrap(),
            std::slice::from_ref(&w.window_id)
        );
        assert_eq!(code(l.window_ref(&w.window_id)), ErrorCode::WINDOW_NOT_FOUND);
    }

    #[test]
    fn restore_brings_back_a_snapshot() {
        let mut l = Layout::default();
        let before = l.snapshot();
        l.resolve_window(None).unwrap();
        l.restore(before);
        assert!(l.list().is_empty());
    }
}
