//! pane の作成・ラベル・close・デーモン終了時の片付け（Node 版の `pane-lifecycle`）。
//! 状態を変えてから保存し、失敗したら戻す。変更は `gate` で 1 つずつ行い、イベントは保存してから出す。

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use misao_protocol::ErrorCode;
use serde_json::{Map, Value, json};
use tokio::task::JoinSet;

use crate::core::Core;
use crate::layout::ResolvedWindow;
use crate::pane::{Pane, PaneOpenOptions};
use crate::params::{self, PaneOpenParams, PaneRefParams, PaneSetLabelParams};
use crate::registry::{Body, PaneEntry};
use crate::rpc_error::{RpcFailure, RpcResult};

type Raw<'a> = Option<&'a Map<String, Value>>;

fn ok() -> Value {
    json!({"ok": true})
}

impl Core {
    pub(crate) async fn pane_open(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneOpenParams = params::parse("pane.open", raw)?;
        if p.preplace.is_some() {
            return Err(RpcFailure::new(
                ErrorCode::NOT_IMPLEMENTED,
                "preplace is not implemented",
            ));
        }
        let labels = p.labels.unwrap_or_default();
        // 保存するのは env のみ。ephemeralEnv は spawn で使い切り、ここには渡らない。
        let env = p.env.clone().unwrap_or_default();
        let _gate = self.gate.lock().await;
        // layout（既定の遅延作成を含む）→ spawn → 登録 → 保存を 1 つの単位として扱う。失敗したら全て戻し、イベントは出さない。
        let (pane, target, before) = {
            let mut state = self.state();
            let before = state.layout.snapshot();
            let attempt = state
                .layout
                .resolve_window(p.window_id.as_ref().map(params::WindowId::as_str))
                .and_then(|target| {
                    let pane = Pane::spawn(PaneOpenOptions {
                        cmd: p.cmd,
                        cwd: p.cwd,
                        env: p.env.unwrap_or_default(),
                        ephemeral_env: p.ephemeral_env.unwrap_or_default(),
                        cols: p.cols.map(params::Dimension::get),
                        rows: p.rows.map(params::Dimension::get),
                        socket_path: &self.socket_path,
                        raw_ring_bytes: self.limits.raw_ring_bytes,
                        lines_ring_bytes: self.limits.lines_ring_bytes,
                        scrollback: self.limits.scrollback,
                        profiles: self.profiles.clone(),
                        events: Arc::clone(&self.events),
                    })
                    .map_err(|e| RpcFailure::invalid_params(format!("spawn failed: {e}")))?;
                    Ok((pane, target))
                });
            match attempt {
                Ok((pane, target)) => {
                    state.registry.add(PaneEntry {
                        pane_id: pane.id.clone(),
                        window_id: target.location.window.id.clone(),
                        labels: labels.clone(),
                        env,
                        body: Body::Live(Arc::clone(&pane)),
                    });
                    (pane, target, before)
                }
                Err(e) => {
                    state.layout.restore(before);
                    return Err(e);
                }
            }
        };
        if let Err(e) = self.persist() {
            {
                let mut state = self.state();
                state.layout.restore(before);
                state.registry.remove(&pane.id);
            }
            pane.abort_open().await;
            return Err(e);
        }
        self.announce_opened(&pane, &labels, &target);
        pane.start();
        self.sampler.register(&pane);
        Ok(json!({"paneId": pane.id}))
    }

    /// 既定の workspace / window の作成と `pane.opened` を出す。
    fn announce_opened(&self, pane: &Pane, labels: &BTreeMap<String, String>, target: &ResolvedWindow) {
        let workspace = &target.location.workspace;
        let window = &target.location.window;
        if target.created_workspace {
            self.events
                .emit("workspace.created", json!({"name": workspace}), None);
        }
        if target.created_window {
            self.events.emit(
                "window.created",
                json!({"windowId": window.id, "workspace": workspace, "name": window.name}),
                None,
            );
        }
        self.events.emit(
            "pane.opened",
            json!({
                "pid": pane.pid,
                "cmd": pane.cmd,
                "labels": labels,
                "workspace": workspace,
                "windowId": window.id,
            }),
            Some(&pane.id),
        );
    }

    pub(crate) async fn pane_set_label(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneSetLabelParams = params::parse("pane.set_label", raw)?;
        p.require_change("pane.set_label")?;
        let pane_id = p.pane_id.as_str();
        let _gate = self.gate.lock().await;
        let (before, labels) = {
            let mut state = self.state();
            let before = state.registry.get_or_throw(pane_id)?.labels.clone();
            let labels = state
                .registry
                .set_labels(pane_id, p.set.as_ref(), p.unset.as_deref())?;
            (before, labels)
        };
        // 保存に失敗したらラベルを元に戻す。
        if let Err(e) = self.persist() {
            self.state().registry.replace_labels(pane_id, before);
            return Err(e);
        }
        self.events.emit(
            "pane.label",
            json!({"set": p.set.unwrap_or_default(), "unset": p.unset.unwrap_or_default()}),
            Some(pane_id),
        );
        Ok(json!({"labels": labels}))
    }

    pub(crate) async fn pane_close(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneRefParams = params::parse("pane.close", raw)?;
        self.close_pane(p.pane_id.as_str()).await?;
        Ok(ok())
    }

    /// pane.close と同じ手順で閉じる（終了を待ち、登録を外し、保存して `pane.closed` を出す）。
    /// live はプロセスを戻せないので、保存に失敗してもメモリは実態（閉じ済み）のままにして、`pane.closed` を出してから失敗を返す。
    /// stopped は記録を消すだけで、保存に失敗したら記録を戻す。
    pub(crate) async fn close_pane(&self, pane_id: &str) -> RpcResult<()> {
        let live = {
            let state = self.state();
            match &state.registry.get_or_throw(pane_id)?.body {
                Body::Live(pane) => Some(Arc::clone(pane)),
                Body::Stopped(_) => None,
            }
        };
        let Some(pane) = live else {
            return self.close_stopped(pane_id).await;
        };
        pane.close().await;
        let _gate = self.gate.lock().await;
        {
            // 並行した close が後始末済み
            let state = self.state();
            if !state
                .registry
                .get(pane_id)
                .and_then(PaneEntry::live)
                .is_some_and(|live| Arc::ptr_eq(live, &pane))
            {
                return Ok(());
            }
        }
        self.release_attachments(&pane); // client.detached は pane.closed より前に出す
        self.state().registry.remove(pane_id);
        pane.release();
        let saved = self.persist();
        self.events.emit("pane.closed", json!({}), Some(pane_id));
        saved
    }

    async fn close_stopped(&self, pane_id: &str) -> RpcResult<()> {
        let _gate = self.gate.lock().await;
        let removed = {
            let mut state = self.state();
            let index = state.registry.index_of(pane_id);
            index.and_then(|i| state.registry.remove(pane_id).map(|entry| (i, entry)))
        };
        let Some((index, entry)) = removed else {
            return Ok(()); // 並行した close が後始末済み
        };
        if let Err(e) = self.persist() {
            self.state().registry.insert_at(index, entry);
            return Err(e);
        }
        self.events.emit("pane.closed", json!({}), Some(pane_id));
        Ok(())
    }

    /// 配下の pane を全部閉じる。閉じている間に開かれた pane も残さない。
    pub(crate) async fn close_panes_in(self: &Arc<Self>, window_ids: &HashSet<String>) -> RpcResult<()> {
        loop {
            let ids: Vec<String> = self
                .state()
                .registry
                .filter(None, None, Some(window_ids))
                .into_iter()
                .map(|e| e.pane_id.clone())
                .collect();
            if ids.is_empty() {
                return Ok(());
            }
            let mut closing = JoinSet::new();
            for id in ids {
                let core = Arc::clone(self);
                closing.spawn(async move { core.close_pane(&id).await });
            }
            while let Some(result) = closing.join_next().await {
                result.map_err(|e| RpcFailure::internal(e.to_string()))??;
            }
        }
    }

    /// デーモンの終了。生きている pane を全部閉じる（`pane.closed` は出さない: 再起動後も pane の記録は残る）。
    pub async fn shutdown(&self) {
        // 記録は消さない（再起動後に stopped として復元される）。遅らせていたサイズの保存だけを済ませる。
        {
            let gate = self.gate.lock().await;
            self.flush_persist(&gate);
        }
        let panes = self.state().registry.panes();
        let mut closing = JoinSet::new();
        for pane in panes {
            closing.spawn(async move {
                pane.close().await;
                pane.release();
            });
        }
        while closing.join_next().await.is_some() {}
    }
}
