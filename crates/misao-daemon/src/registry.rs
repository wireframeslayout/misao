use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use misao_protocol::ErrorCode;

use crate::pane::{Pane, RunState};
use crate::params::{Dimension, Labels, PaneId, ProcessState, WindowId};
use crate::persistence::PersistedPane;
use crate::rpc_error::{RpcFailure, RpcResult};

/// 再起動後に復元した、実行体の無い pane（`stopped`）の記録。
#[derive(Debug, Clone)]
pub struct StoppedPane {
    pub cmd: Vec<String>,
    pub cwd: String,
    pub cols: u64,
    pub rows: u64,
}

pub enum Body {
    Live(Arc<Pane>),
    Stopped(StoppedPane),
}

/// ペインのメタ（所属の window とラベル、保存する env）と実行体の対応。
pub struct PaneEntry {
    pub pane_id: String,
    pub window_id: String,
    pub labels: Labels,
    /// `pane.open` の env のみ（`ephemeralEnv` は持たない）。
    pub env: BTreeMap<String, String>,
    pub body: Body,
}

impl PaneEntry {
    pub fn live(&self) -> Option<&Arc<Pane>> {
        match &self.body {
            Body::Live(pane) => Some(pane),
            Body::Stopped(_) => None,
        }
    }
}

/// ペインの一覧（作成順）。親（window / workspace）の名前は知らない。
#[derive(Default)]
pub struct PaneRegistry {
    entries: Vec<PaneEntry>,
}

impl PaneRegistry {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn add(&mut self, entry: PaneEntry) {
        self.entries.push(entry);
    }

    /// 起動時の復元。実行体は無く、`stopped` として扱う。
    pub fn restore_stopped(&mut self, panes: Vec<PersistedPane>) {
        for p in panes {
            self.entries.push(PaneEntry {
                pane_id: String::from(p.pane_id),
                window_id: String::from(p.window_id),
                labels: p.labels,
                env: p.env,
                body: Body::Stopped(StoppedPane {
                    cmd: p.cmd,
                    cwd: p.cwd,
                    cols: u64::from(p.cols),
                    rows: u64::from(p.rows),
                }),
            });
        }
    }

    pub fn get(&self, pane_id: &str) -> Option<&PaneEntry> {
        self.entries.iter().find(|e| e.pane_id == pane_id)
    }

    pub fn get_or_throw(&self, pane_id: &str) -> RpcResult<&PaneEntry> {
        self.get(pane_id)
            .ok_or_else(|| RpcFailure::new(ErrorCode::PANE_NOT_FOUND, format!("pane not found: {pane_id}")))
    }

    /// 実行体のある pane。stopped（再起動後の復元分）は PaneExited。
    pub fn live(&self, pane_id: &str) -> RpcResult<Arc<Pane>> {
        match &self.get_or_throw(pane_id)?.body {
            Body::Live(pane) => Ok(Arc::clone(pane)),
            Body::Stopped(_) => Err(RpcFailure::new(ErrorCode::PANE_EXITED, "pane is stopped")),
        }
    }

    pub fn remove(&mut self, pane_id: &str) -> Option<PaneEntry> {
        let index = self.entries.iter().position(|e| e.pane_id == pane_id)?;
        Some(self.entries.remove(index))
    }

    /// 元の位置に戻す（保存に失敗した close の取り消し）。
    pub fn insert_at(&mut self, index: usize, entry: PaneEntry) {
        self.entries.insert(index.min(self.entries.len()), entry);
    }

    pub fn index_of(&self, pane_id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.pane_id == pane_id)
    }

    /// set → unset の順に適用した新しい labels を返し、entry を置き換える。
    pub fn set_labels(
        &mut self,
        pane_id: &str,
        set: Option<&Labels>,
        unset: Option<&[String]>,
    ) -> RpcResult<Labels> {
        self.get_or_throw(pane_id)?;
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.pane_id == pane_id)
            .ok_or_else(|| RpcFailure::internal("pane vanished while labelling"))?;
        let mut labels = entry.labels.clone();
        labels.extend(set.into_iter().flatten().map(|(k, v)| (k.clone(), v.clone())));
        for key in unset.unwrap_or_default() {
            labels.remove(key);
        }
        entry.labels = labels.clone();
        Ok(labels)
    }

    pub fn replace_labels(&mut self, pane_id: &str, labels: Labels) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.pane_id == pane_id) {
            entry.labels = labels;
        }
    }

    /// 全条件の AND。`window_ids` が None なら window では絞らない。
    pub fn filter(
        &self,
        state: Option<ProcessState>,
        labels: Option<&Labels>,
        window_ids: Option<&HashSet<String>>,
    ) -> Vec<&PaneEntry> {
        self.entries
            .iter()
            .filter(|e| match (state, &e.body) {
                (None, _) => true,
                (Some(ProcessState::Running), Body::Live(pane)) => pane.state() == RunState::Running,
                (Some(ProcessState::Exited), Body::Live(pane)) => pane.state() == RunState::Exited,
                (Some(ProcessState::Stopped), Body::Stopped(_)) => true,
                _ => false,
            })
            .filter(|e| labels.is_none_or(|want| want.iter().all(|(k, v)| e.labels.get(k) == Some(v))))
            .filter(|e| window_ids.is_none_or(|ids| ids.contains(&e.window_id)))
            .collect()
    }

    pub fn panes(&self) -> Vec<Arc<Pane>> {
        self.entries
            .iter()
            .filter_map(|e| e.live().map(Arc::clone))
            .collect()
    }

    /// 保存用の記録。live の現在の cols / rows を反映する。
    pub fn to_persisted(&self) -> Result<Vec<PersistedPane>, String> {
        self.entries.iter().map(persisted).collect()
    }
}

fn persisted(entry: &PaneEntry) -> Result<PersistedPane, String> {
    let (cmd, cwd, cols, rows) = match &entry.body {
        Body::Live(pane) => {
            let (cols, rows) = pane.size();
            (pane.cmd.clone(), pane.cwd.clone(), cols, rows)
        }
        Body::Stopped(s) => (s.cmd.clone(), s.cwd.clone(), s.cols, s.rows),
    };
    let dimension = |n: u64| Dimension::try_from(serde_json::Number::from(n));
    Ok(PersistedPane {
        pane_id: PaneId::try_from(entry.pane_id.clone())?,
        window_id: WindowId::try_from(entry.window_id.clone())?,
        cmd,
        cwd,
        env: entry.env.clone(),
        labels: entry.labels.clone(),
        cols: dimension(cols)?,
        rows: dimension(rows)?,
    })
}
