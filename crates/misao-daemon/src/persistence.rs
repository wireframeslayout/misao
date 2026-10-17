//! 状態の保存形式（`persistence.json`）。Node 版（`packages/daemon/src/persistence.ts`）と同じファイルを読み書きできる。
//! 保存するのは workspace / window の定義と pane のメタだけ。`ephemeralEnv` は保存しない（`env` は `pane.open` の env のみ）。

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::params::{Dimension, Labels, Name, PaneId, WindowId};

/// `version` は 1 だけを受ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Version1;

impl TryFrom<u8> for Version1 {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, String> {
        if value == 1 {
            Ok(Self)
        } else {
            Err(format!("invalid value {value}: expected 1"))
        }
    }
}

impl From<Version1> for u8 {
    fn from(_: Version1) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedWindow {
    pub id: WindowId,
    pub name: Name,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedWorkspace {
    pub name: Name,
    pub windows: Vec<PersistedWindow>,
}

/// pane のメタ。`env` は `pane.open` の env のみで、`ephemeralEnv` は持たない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PersistedPane {
    pub pane_id: PaneId,
    pub window_id: WindowId,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub labels: Labels,
    pub cols: Dimension,
    pub rows: Dimension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedState {
    pub version: Version1,
    pub workspaces: Vec<PersistedWorkspace>,
    pub panes: Vec<PersistedPane>,
}

impl PersistedState {
    pub fn empty() -> Self {
        Self {
            version: Version1,
            workspaces: Vec::new(),
            panes: Vec::new(),
        }
    }

    /// workspace 名・window ID・pane ID の重複と、存在しない window への所属。問題が無ければ空。
    pub fn consistency_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let window_ids: HashSet<&str> = self
            .workspaces
            .iter()
            .flat_map(|w| w.windows.iter().map(|win| win.id.as_str()))
            .collect();
        let window_count: usize = self.workspaces.iter().map(|w| w.windows.len()).sum();
        let unique = |items: Vec<&str>| items.iter().collect::<HashSet<_>>().len() == items.len();
        if !unique(self.workspaces.iter().map(|w| w.name.as_str()).collect()) {
            issues.push("duplicate workspace name".to_string());
        }
        if window_ids.len() != window_count {
            issues.push("duplicate window id".to_string());
        }
        if !unique(self.panes.iter().map(|p| p.pane_id.as_str()).collect()) {
            issues.push("duplicate pane id".to_string());
        }
        for p in &self.panes {
            if p.cmd.is_empty() {
                issues.push(format!("pane {}: cmd must not be empty", p.pane_id.as_str()));
            }
            if !window_ids.contains(p.window_id.as_str()) {
                issues.push(format!(
                    "pane {}: unknown window {}",
                    p.pane_id.as_str(),
                    p.window_id.as_str()
                ));
            }
        }
        issues
    }
}

/// ファイルが無ければ空の状態。壊れていたり未知の version なら Err（起動を止める）。
pub fn load_persisted_state(path: &Path) -> Result<PersistedState, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(PersistedState::empty()),
        Err(e) => return Err(e.to_string()),
    };
    let invalid = |detail: String| format!("invalid persistence file {}: {detail}", path.display());
    let state: PersistedState = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
    let issues = state.consistency_issues();
    if !issues.is_empty() {
        return Err(invalid(format!("state: {}", issues.join("; "))));
    }
    Ok(state)
}

/// 一時ファイルに書いて fsync してから rename する（途中で落ちても、電源断でも、元のファイルは壊れない）。
pub fn save_persisted_state(path: &Path, state: &PersistedState) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    let written = write_synced(&tmp, state).and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn write_synced(tmp: &Path, state: &PersistedState) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(tmp)?;
    let mut text = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    text.push('\n');
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const WIN: &str = "w_01M3AAAAAAAAAAAAAAAAAAAAAA";
    const PANE: &str = "p_01M3AAAAAAAAAAAAAAAAAAAAAA";

    fn sample() -> String {
        format!(
            r#"{{
  "version": 1,
  "workspaces": [{{"name": "default", "windows": [{{"id": "{WIN}", "name": "default"}}]}}],
  "panes": [{{"paneId": "{PANE}", "windowId": "{WIN}", "cmd": ["sh"], "cwd": "/tmp",
             "env": {{"A": "1"}}, "labels": {{"task": "7"}}, "cols": 80, "rows": 24}}]
}}"#
        )
    }

    #[test]
    fn reads_a_file_in_the_node_format_and_writes_it_back_readably() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("persistence.json");
        fs::write(&path, sample()).unwrap();
        let state = load_persisted_state(&path).unwrap();
        assert_eq!((state.workspaces.len(), state.panes.len()), (1, 1));
        let again = dir.path().join("again.json");
        save_persisted_state(&again, &state).unwrap();
        assert_eq!(load_persisted_state(&again).unwrap(), state);
        let text = fs::read_to_string(&again).unwrap();
        assert!(
            text.ends_with("}\n") && text.starts_with("{\n  \"version\": 1,"),
            "{text}"
        );
        assert!(!text.contains("ephemeralEnv"));
    }

    #[test]
    fn a_missing_file_is_an_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let state = load_persisted_state(&dir.path().join("none.json")).unwrap();
        assert_eq!(state, PersistedState::empty());
    }

    #[test]
    fn broken_files_are_rejected_with_the_path_in_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("persistence.json");
        for bad in [
            "not json".to_string(),
            sample().replace("\"version\": 1", "\"version\": 2"),
            sample().replace("\"cols\": 80", "\"cols\": 0"),
            sample().replace("\"rows\": 24", "\"rows\": 24, \"extra\": 1"),
            sample().replace(
                &format!("\"windowId\": \"{WIN}\""),
                "\"windowId\": \"w_01M3BBBBBBBBBBBBBBBBBBBBBB\"",
            ),
        ] {
            fs::write(&path, &bad).unwrap();
            let message = load_persisted_state(&path).unwrap_err();
            assert!(
                message.starts_with(&format!("invalid persistence file {}", path.display())),
                "{message}"
            );
        }
    }

    #[test]
    fn duplicates_are_reported_as_issues() {
        let mut state: PersistedState = serde_json::from_str(&sample()).unwrap();
        state.workspaces.push(state.workspaces[0].clone());
        state.panes.push(state.panes[0].clone());
        let issues = state.consistency_issues();
        assert!(
            issues.contains(&"duplicate workspace name".to_string()),
            "{issues:?}"
        );
        assert!(issues.contains(&"duplicate window id".to_string()));
        assert!(issues.contains(&"duplicate pane id".to_string()));
    }

    #[test]
    fn saving_leaves_no_temporary_file_and_uses_mode_600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("persistence.json");
        save_persisted_state(&path, &PersistedState::empty()).unwrap();
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["persistence.json".to_string()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
