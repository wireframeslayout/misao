//! 擬似端末の子プロセス（portable-pty）の起動。終了待ちとプロセスツリーの停止は OS ごとに違うので、
//! `ProcessTree`（Unix はプロセスグループ、Windows は Job Object）に閉じ込める。

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

#[cfg(unix)]
#[path = "pty_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "pty_windows.rs"]
mod platform;

pub use platform::{LEADER_HELD, ProcessTree};

/// 終了した子の状態。シグナルで死んだときは `exit_code` が None（Node 版と同じ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

/// プロセスグループ（Windows では Job Object）にまだメンバーがいるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    Alive,
    Empty,
    /// 調べられなかった。いないとは言えないので、呼び出し側は安全側（期限まで待って SIGKILL）に倒す。
    Unknown,
}

pub struct SpawnRequest<'a> {
    pub cmd: &'a [String],
    pub cwd: &'a str,
    pub env: Vec<(OsString, OsString)>,
    pub cols: u64,
    pub rows: u64,
}

pub struct PtyProcess {
    pub pid: u32,
    pub tree: ProcessTree,
    pub master: Box<dyn MasterPty + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

/// 端末のサイズ。pty の寸法は 16 bit なので、pty へ渡すときだけ上限に丸める（報告する値は元のまま）。
pub fn pty_size(cols: u64, rows: u64) -> PtySize {
    let clamp = |n: u64| u16::try_from(n).unwrap_or(u16::MAX);
    PtySize {
        rows: clamp(rows),
        cols: clamp(cols),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// 起動した子が、`spawn` の後続の失敗で孤児にならないための番人。破棄されるとプロセスツリーごと止めて回収する。
struct ChildGuard {
    child: Option<Box<dyn Child + Send + Sync>>,
    tree: Option<ProcessTree>,
}

impl ChildGuard {
    fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self {
            child: Some(child),
            tree: None,
        }
    }

    fn disarm(mut self) -> Option<Box<dyn Child + Send + Sync>> {
        self.child.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        match &self.tree {
            Some(tree) => tree.kill(),
            None => {
                let _ = child.kill();
            }
        }
        let _ = child.wait();
    }
}

/// 起動するコマンドの argv。Windows では `PATHEXT` と `.ps1` を解決する（Unix は portable-pty が PATH を探す）。
#[cfg(windows)]
fn launch_argv(req: &SpawnRequest<'_>) -> Result<Vec<String>, String> {
    crate::cmd_resolve::resolve_command(
        req.cmd,
        &crate::cmd_resolve::WindowsEnv::from_pairs(&req.env, req.cwd),
    )
}

#[cfg(not(windows))]
#[allow(clippy::unnecessary_wraps)]
fn launch_argv(req: &SpawnRequest<'_>) -> Result<Vec<String>, String> {
    Ok(req.cmd.to_vec())
}

/// 子プロセスを擬似端末で起動する。失敗の文字列は `pane.open` の InvalidParams（`spawn failed: ...`）に使う。
/// reader と writer は子を起動する前に用意するので、起動後に失敗しうるのは pid の取得と、プロセスツリーへの登録だけで、
/// そのときも子を止めて回収する。
pub fn spawn(req: &SpawnRequest<'_>) -> Result<PtyProcess, String> {
    if req.cmd.is_empty() {
        return Err("cmd is empty".to_string());
    }
    // portable-pty は cwd がディレクトリでないとき黙ってホームに切り替えるので、先に確かめる。
    if !Path::new(req.cwd).is_dir() {
        return Err(format!("cwd is not a directory: {}", req.cwd));
    }
    let argv = launch_argv(req)?;
    let (program, args) = argv.split_first().ok_or("cmd is empty")?;
    let pair = native_pty_system()
        .openpty(pty_size(req.cols, req.rows))
        .map_err(|e| format!("{e:#}"))?;
    let reader = pair.master.try_clone_reader().map_err(|e| format!("{e:#}"))?;
    let writer = pair.master.take_writer().map_err(|e| format!("{e:#}"))?;
    let mut builder = CommandBuilder::new(program);
    builder.args(args);
    builder.cwd(req.cwd);
    builder.env_clear();
    for (key, value) in &req.env {
        builder.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| format!("{}: {e:#}", req.cmd[0]))?;
    let mut guard = ChildGuard::new(child);
    // slave を親が持ち続けると、子が終わっても master が EOF にならない。
    drop(pair.slave);
    let pid = guard
        .child
        .as_ref()
        .and_then(|child| child.process_id())
        .ok_or("the child has no process id")?;
    let tree = match guard.child.as_deref() {
        Some(child) => ProcessTree::attach(child, pid)?,
        None => return Err("the child has no process id".to_string()),
    };
    guard.tree = Some(tree.clone());
    let child = guard.disarm().ok_or("the child has no process id")?;
    Ok(PtyProcess {
        pid,
        tree,
        master: pair.master,
        reader,
        writer,
        child,
    })
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn request(cmd: &[String]) -> SpawnRequest<'_> {
        SpawnRequest {
            cmd,
            cwd: "/",
            env: std::env::vars_os().collect(),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn dropping_the_guard_kills_and_reaps_the_child() {
        let cmd = vec!["sleep".to_string(), "300".to_string()];
        let process = spawn(&request(&cmd)).unwrap();
        let tree = process.tree.clone();
        assert_eq!(tree.group_state(), GroupState::Alive);
        let mut guard = ChildGuard::new(process.child);
        guard.tree = Some(process.tree);
        drop(guard);
        // 回収済み（zombie でもない）なので、グループごと消えている。
        // (Drop の `child.wait()` は portable-pty が回収する。)
        assert_eq!(tree.group_state(), GroupState::Empty);
    }

    #[test]
    fn a_disarmed_guard_leaves_the_child_running() {
        let cmd = vec!["sleep".to_string(), "300".to_string()];
        let process = spawn(&request(&cmd)).unwrap();
        let tree = process.tree.clone();
        let child = ChildGuard::new(process.child).disarm();
        assert!(child.is_some() && tree.group_state() == GroupState::Alive);
        tree.kill();
        let _ = tree.wait_exit(child.unwrap());
    }

    #[test]
    fn spawn_fails_cleanly_for_a_missing_command_or_directory() {
        let missing = vec!["/nonexistent/command".to_string()];
        assert!(spawn(&request(&missing)).is_err());
        let cmd = vec!["true".to_string()];
        let mut bad_cwd = request(&cmd);
        bad_cwd.cwd = "/nonexistent/dir";
        assert!(spawn(&bad_cwd).is_err());
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod reap_tests {
    use super::*;

    #[test]
    fn reaping_collects_a_held_leader_and_treats_an_already_reaped_one_as_done() {
        let cmd = vec!["true".to_string()];
        let process = spawn(&SpawnRequest {
            cmd: &cmd,
            cwd: "/",
            env: std::env::vars_os().collect(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        let exit = process.tree.wait_exit(process.child);
        assert_eq!(exit.exit_code, Some(0));
        assert!(process.tree.reap().is_ok());
        assert!(
            process.tree.reap().is_ok(),
            "ECHILD after the first reap is success"
        );
    }
}
