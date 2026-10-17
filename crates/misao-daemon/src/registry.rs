use std::collections::HashSet;
use std::sync::Arc;

use misao_protocol::ErrorCode;

use crate::pane::{Pane, RunState};
use crate::params::{Labels, ProcessState};
use crate::rpc_error::{RpcFailure, RpcResult};

/// ペインのメタ（所属の window とラベル）と実行体の対応。
pub struct PaneEntry {
    pub pane: Arc<Pane>,
    pub window_id: String,
    pub labels: Labels,
}

/// 実行中のペインの一覧（作成順）。親（window / workspace）の名前は知らない。
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

    pub fn get(&self, pane_id: &str) -> Option<&PaneEntry> {
        self.entries.iter().find(|e| e.pane.id == pane_id)
    }

    pub fn get_or_throw(&self, pane_id: &str) -> RpcResult<&PaneEntry> {
        self.get(pane_id)
            .ok_or_else(|| RpcFailure::new(ErrorCode::PANE_NOT_FOUND, format!("pane not found: {pane_id}")))
    }

    pub fn remove(&mut self, pane_id: &str) -> Option<PaneEntry> {
        let index = self.entries.iter().position(|e| e.pane.id == pane_id)?;
        Some(self.entries.remove(index))
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
            .find(|e| e.pane.id == pane_id)
            .ok_or_else(|| RpcFailure::internal("pane vanished while labelling"))?;
        let mut labels = entry.labels.clone();
        labels.extend(set.into_iter().flatten().map(|(k, v)| (k.clone(), v.clone())));
        for key in unset.unwrap_or_default() {
            labels.remove(key);
        }
        entry.labels = labels.clone();
        Ok(labels)
    }

    /// 全条件の AND。`window_ids` が None なら window では絞らない。`stopped`（再起動後の復元）はまだ無い。
    pub fn filter(
        &self,
        state: Option<ProcessState>,
        labels: Option<&Labels>,
        window_ids: Option<&HashSet<String>>,
    ) -> Vec<&PaneEntry> {
        self.entries
            .iter()
            .filter(|e| match state {
                None => true,
                Some(ProcessState::Running) => e.pane.state() == RunState::Running,
                Some(ProcessState::Exited) => e.pane.state() == RunState::Exited,
                Some(ProcessState::Stopped) => false,
            })
            .filter(|e| labels.is_none_or(|want| want.iter().all(|(k, v)| e.labels.get(k) == Some(v))))
            .filter(|e| window_ids.is_none_or(|ids| ids.contains(&e.window_id)))
            .collect()
    }

    pub fn panes(&self) -> Vec<Arc<Pane>> {
        self.entries.iter().map(|e| Arc::clone(&e.pane)).collect()
    }
}
