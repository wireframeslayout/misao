//! workspace.* / window.* の RPC（Node 版の `layout-handlers`）。
//! 作成・rename・close は layout を変えてからイベントを出す。

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::core::{Core, to_value};
use crate::params::{
    self, Empty, WindowCreateParams, WindowIdParams, WindowRenameParams, WorkspaceNameParams,
    WorkspaceRenameParams,
};
use crate::rpc_error::RpcResult;

type Raw<'a> = Option<&'a Map<String, Value>>;

fn ok() -> Value {
    json!({"ok": true})
}

impl Core {
    pub(crate) fn workspace_list(&self, raw: Raw<'_>) -> RpcResult<Value> {
        params::parse::<Empty>("workspace.list", raw)?;
        to_value(&self.state().layout.list())
    }

    pub(crate) fn workspace_create(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: WorkspaceNameParams = params::parse("workspace.create", raw)?;
        let info = self.state().layout.create_workspace(p.name.as_str())?;
        self.events
            .emit("workspace.created", json!({"name": p.name.as_str()}), None);
        to_value(&info)
    }

    pub(crate) fn workspace_rename(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: WorkspaceRenameParams = params::parse("workspace.rename", raw)?;
        self.state()
            .layout
            .rename_workspace(p.name.as_str(), p.new_name.as_str())?;
        self.events.emit(
            "workspace.renamed",
            json!({"name": p.name.as_str(), "newName": p.new_name.as_str()}),
            None,
        );
        Ok(ok())
    }

    pub(crate) async fn workspace_close(self: &Arc<Self>, raw: Raw<'_>) -> RpcResult<Value> {
        let p: WorkspaceNameParams = params::parse("workspace.close", raw)?;
        let name = p.name.as_str();
        // 閉じている間は layout が closing として配下への作成・pane.open を拒否する。pane を閉じるのに失敗したら取り消す。
        let window_ids = self.state().layout.begin_close_workspace(name)?;
        if let Err(e) = self.close_panes_in(&window_ids.iter().cloned().collect()).await {
            self.state().layout.cancel_close_workspace(name);
            return Err(e);
        }
        self.state().layout.close_workspace(name)?;
        for window_id in window_ids {
            self.events
                .emit("window.closed", json!({"windowId": window_id}), None);
        }
        self.events.emit("workspace.closed", json!({"name": name}), None);
        Ok(ok())
    }

    pub(crate) fn window_create(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: WindowCreateParams = params::parse("window.create", raw)?;
        let info = self
            .state()
            .layout
            .create_window(p.workspace.as_str(), p.name.as_str())?;
        self.events.emit(
            "window.created",
            json!({"windowId": info.window_id, "workspace": p.workspace.as_str(), "name": p.name.as_str()}),
            None,
        );
        to_value(&info)
    }

    pub(crate) fn window_rename(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: WindowRenameParams = params::parse("window.rename", raw)?;
        self.state()
            .layout
            .rename_window(p.window_id.as_str(), p.name.as_str())?;
        self.events.emit(
            "window.renamed",
            json!({"windowId": p.window_id.as_str(), "name": p.name.as_str()}),
            None,
        );
        Ok(ok())
    }

    pub(crate) async fn window_close(self: &Arc<Self>, raw: Raw<'_>) -> RpcResult<Value> {
        let p: WindowIdParams = params::parse("window.close", raw)?;
        let window_id = p.window_id.as_str();
        self.state().layout.begin_close_window(window_id)?;
        if let Err(e) = self.close_panes_in(&HashSet::from([window_id.to_string()])).await {
            self.state().layout.cancel_close_window(window_id);
            return Err(e);
        }
        self.state().layout.close_window(window_id)?;
        self.events
            .emit("window.closed", json!({"windowId": window_id}), None);
        Ok(ok())
    }
}
