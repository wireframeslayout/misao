//! 状態の保存形式（`persistence.json`）。Node 版（`packages/daemon/src/persistence.ts`）と同じファイルを読み書きできる。
//! 保存するのは workspace / window の定義と pane のメタだけ。`ephemeralEnv` は保存しない（`env` は `pane.open` の env のみ）。

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::params::{Dimension, Labels, Name, PaneId, WindowId};

/// 読み込む `persistence.json` の最大サイズ。これを超えるファイルは起動エラーにする（読む前に長さで弾く）。
pub const MAX_STATE_BYTES: u64 = 32 * 1024 * 1024;
/// workspace・window・pane それぞれの最大数。
const MAX_ENTRIES: usize = 10_000;
/// 配列（`cmd`）と map（`env`・`labels`）の最大要素数。
const MAX_ITEMS: usize = 4096;
/// 文字列（名前・`cwd`・引数・env・ラベルのキーと値）の最大の長さ（バイト）。プロトコルの 1 行（8 MiB）より十分小さい。
const MAX_STRING_BYTES: usize = 1024 * 1024;

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

    /// 数と長さの上限の違反。保存するときも同じ上限で断る（読み込めないファイルを作らない）。
    fn limit_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let window_count: usize = self.workspaces.iter().map(|w| w.windows.len()).sum();
        for (what, count) in [
            ("workspaces", self.workspaces.len()),
            ("windows", window_count),
            ("panes", self.panes.len()),
        ] {
            if count > MAX_ENTRIES {
                issues.push(format!("too many {what}: {count} (max {MAX_ENTRIES})"));
            }
        }
        let long = |s: &str| s.len() > MAX_STRING_BYTES;
        for w in &self.workspaces {
            if long(w.name.as_str()) || w.windows.iter().any(|win| long(win.name.as_str())) {
                issues.push(format!(
                    "workspace {}: a name is too long",
                    w.name.as_str().chars().take(32).collect::<String>()
                ));
            }
        }
        for p in &self.panes {
            let id = p.pane_id.as_str();
            let too_many = p.cmd.len() > MAX_ITEMS || p.env.len() > MAX_ITEMS || p.labels.len() > MAX_ITEMS;
            let too_long = p.cmd.iter().any(|a| long(a))
                || long(&p.cwd)
                || p.env.iter().any(|(k, v)| long(k) || long(v))
                || p.labels.iter().any(|(k, v)| long(k) || long(v));
            if too_many {
                issues.push(format!(
                    "pane {id}: too many cmd / env / labels entries (max {MAX_ITEMS})"
                ));
            }
            if too_long {
                issues.push(format!(
                    "pane {id}: a string is longer than {MAX_STRING_BYTES} bytes"
                ));
            }
        }
        issues
    }

    /// workspace 名・window ID・pane ID の重複と、存在しない window への所属、上限の違反。問題が無ければ空。
    pub fn consistency_issues(&self) -> Vec<String> {
        let mut issues = self.limit_issues();
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

/// ファイルが無ければ空の状態。壊れていたり未知の version、通常のファイルでない（シンボリックリンクを含む）、
/// 大きすぎる、上限を超えている場合は Err（起動を止める）。
pub fn load_persisted_state(path: &Path) -> Result<PersistedState, String> {
    let invalid = |detail: String| format!("invalid persistence file {}: {detail}", path.display());
    let mut file = match open_for_read(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(PersistedState::empty()),
        Err(e) => return Err(invalid(e.to_string())),
    };
    let meta = file.metadata().map_err(|e| invalid(e.to_string()))?;
    if !meta.is_file() {
        return Err(invalid("not a regular file".to_string()));
    }
    if meta.len() > MAX_STATE_BYTES {
        return Err(invalid(format!(
            "file is too large: {} bytes (max {MAX_STATE_BYTES})",
            meta.len()
        )));
    }
    // 読んでいる間に伸びても上限を超えて読まない。
    let mut text = String::new();
    (&mut file)
        .take(MAX_STATE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| invalid(e.to_string()))?;
    if text.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid(format!(
            "file is too large (max {MAX_STATE_BYTES} bytes)"
        )));
    }
    let state: PersistedState = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
    let issues = state.consistency_issues();
    if !issues.is_empty() {
        return Err(invalid(format!("state: {}", issues.join("; "))));
    }
    Ok(state)
}

/// シンボリックリンクは辿らない（O_NOFOLLOW）。
fn open_for_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, nix::fcntl::OFlag::O_NOFOLLOW.bits());
    options.open(path)
}

/// 保存の失敗。rename より前なら元のファイルは無傷（呼び出し側は変更を戻す）、後ならファイルは新しい内容になっている
/// （durability だけの失敗: 戻してはいけない）。
#[derive(Debug)]
pub enum SaveError {
    BeforeRename(io::Error),
    AfterRename(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeRename(e) => write!(f, "{e}"),
            Self::AfterRename(e) => write!(
                f,
                "the file was replaced but its directory could not be synced: {e}"
            ),
        }
    }
}

/// 同じディレクトリにランダムな名前の一時ファイルを排他的に（O_EXCL、シンボリックリンクを辿らない）作り、
/// 書く -> ファイルを fsync -> rename -> 親ディレクトリを fsync する（途中で落ちても、電源断でも、元のファイルは壊れない）。
/// ディレクトリの fsync が使えないファイルシステム（EINVAL / ENOTSUP）では、黙って成功にする。
pub fn save_persisted_state(path: &Path, state: &PersistedState) -> Result<(), SaveError> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", crate::ids::new_ulid()));
    let written = write_synced(&tmp, state).and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(SaveError::BeforeRename(e));
    }
    classify_dir_sync(sync_dir(dir)).map_err(SaveError::AfterRename)
}

/// ディレクトリの fsync を持たないファイルシステムの「できない」は、失敗として扱わない。
fn classify_dir_sync(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(e) if is_sync_unsupported(&e) => Ok(()),
        other => other,
    }
}

fn is_sync_unsupported(e: &io::Error) -> bool {
    // EINVAL(22)、ENOTSUP / EOPNOTSUPP(95 Linux、45 / 102 macOS)
    e.kind() == io::ErrorKind::Unsupported
        || e.kind() == io::ErrorKind::InvalidInput
        || matches!(e.raw_os_error(), Some(22 | 95 | 45 | 102))
}

fn write_synced(tmp: &Path, state: &PersistedState) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits());
    }
    let mut file = options.open(tmp)?;
    let mut text = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    text.push('\n');
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// rename を電源断の後も残す。ディレクトリを開いて fsync できない環境（Windows）では何もしない。
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
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
    fn a_symlink_a_directory_and_an_oversized_file_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, sample()).unwrap();
        #[cfg(unix)]
        {
            let link = dir.path().join("persistence.json");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(
                load_persisted_state(&link)
                    .unwrap_err()
                    .starts_with("invalid persistence file")
            );
        }
        let as_dir = dir.path().join("dir.json");
        fs::create_dir(&as_dir).unwrap();
        assert!(load_persisted_state(&as_dir).is_err());
        let big = dir.path().join("big.json");
        fs::write(&big, vec![b' '; usize::try_from(MAX_STATE_BYTES).unwrap() + 1]).unwrap();
        let message = load_persisted_state(&big).unwrap_err();
        assert!(message.contains("too large"), "{message}");
    }

    #[test]
    fn counts_and_lengths_are_limited_on_load_and_on_save() {
        let mut state: PersistedState = serde_json::from_str(&sample()).unwrap();
        assert!(state.consistency_issues().is_empty());
        let pane = state.panes[0].clone();
        state.panes = (0..=MAX_ENTRIES)
            .map(|i| PersistedPane {
                pane_id: PaneId::try_from(format!("p_{i:026}").replace('8', "7").replace('9', "7")).unwrap(),
                ..pane.clone()
            })
            .collect();
        assert!(
            state
                .limit_issues()
                .iter()
                .any(|m| m.starts_with("too many panes"))
        );
        let mut state: PersistedState = serde_json::from_str(&sample()).unwrap();
        state.panes[0].cwd = "x".repeat(MAX_STRING_BYTES + 1);
        state.panes[0].cmd = vec!["a".to_string(); MAX_ITEMS + 1];
        let issues = state.limit_issues();
        assert!(
            issues.iter().any(|m| m.contains("longer than"))
                && issues.iter().any(|m| m.contains("too many cmd")),
            "{issues:?}"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("persistence.json");
        fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
        assert!(load_persisted_state(&path).unwrap_err().contains("state:"));
    }

    #[test]
    fn a_directory_sync_that_the_filesystem_cannot_do_is_not_a_failure_but_other_errors_are() {
        for raw in [22, 95] {
            assert!(
                classify_dir_sync(Err(io::Error::from_raw_os_error(raw))).is_ok(),
                "{raw}"
            );
        }
        assert!(classify_dir_sync(Err(io::Error::from(io::ErrorKind::Unsupported))).is_ok());
        assert!(
            classify_dir_sync(Err(io::Error::from_raw_os_error(5))).is_err(),
            "EIO is a real failure"
        );
        assert!(classify_dir_sync(Ok(())).is_ok());
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
