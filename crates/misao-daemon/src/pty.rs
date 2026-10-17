//! 擬似端末の子プロセス（portable-pty）の起動・終了待ち・プロセスツリーの停止。
//! OS ごとに違う部分（プロセスグループ / Windows のツリー停止）はここに閉じ込める。

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// 終了した子の状態。シグナルで死んだときは `exit_code` が None（Node 版と同じ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

/// プロセスグループにまだメンバーがいるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))] // Windows は taskkill /T で木ごと止めるので Empty しか返さない
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
struct ChildGuard(Option<Box<dyn Child + Send + Sync>>);

impl ChildGuard {
    fn disarm(mut self) -> Option<Box<dyn Child + Send + Sync>> {
        self.0.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            match child.process_id() {
                Some(pid) => kill_tree(pid),
                None => {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        }
    }
}

/// 子プロセスを擬似端末で起動する。失敗の文字列は `pane.open` の InvalidParams（`spawn failed: ...`）に使う。
/// reader と writer は子を起動する前に用意するので、起動後に失敗しうるのは pid の取得だけで、そのときも子を止めて回収する。
pub fn spawn(req: &SpawnRequest<'_>) -> Result<PtyProcess, String> {
    let (program, args) = req.cmd.split_first().ok_or("cmd is empty")?;
    // portable-pty は cwd がディレクトリでないとき黙ってホームに切り替えるので、先に確かめる。
    if !Path::new(req.cwd).is_dir() {
        return Err(format!("cwd is not a directory: {}", req.cwd));
    }
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
    let child = pair.slave.spawn_command(builder).map_err(|e| format!("{e:#}"))?;
    let guard = ChildGuard(Some(child));
    // slave を親が持ち続けると、子が終わっても master が EOF にならない。
    drop(pair.slave);
    let pid = guard
        .0
        .as_ref()
        .and_then(|child| child.process_id())
        .ok_or("the child has no process id")?;
    let child = guard.disarm().ok_or("the child has no process id")?;
    Ok(PtyProcess {
        pid,
        master: pair.master,
        reader,
        writer,
        child,
    })
}

#[cfg(unix)]
mod platform {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};
    use nix::sys::wait::{WaitStatus, waitpid};
    use nix::unistd::Pid;

    use super::{Child, ExitInfo, GroupState};

    fn pgid(pid: u32) -> Option<Pid> {
        i32::try_from(pid).ok().map(Pid::from_raw)
    }

    /// リーダーを `reap` まで回収せずに（ゾンビのまま）残すか。残せば、リーダーの pid（= pgid = sid）は
    /// 回収されるまで再利用されないので、`pane.close` の時点でグループへ signal しても無関係なグループを止めない。
    /// `waitid(WNOWAIT)` と /proc の走査が使える Linux だけ。他の Unix は終了時に回収する。
    pub const LEADER_HELD: bool = cfg!(target_os = "linux");

    /// 終了を待つ。Linux では回収しない（`waitid(WNOWAIT)`）。
    #[cfg(target_os = "linux")]
    fn wait_without_reaping_if_held(pid: Pid) -> nix::Result<WaitStatus> {
        use nix::sys::wait::{Id, WaitPidFlag, waitid};
        waitid(Id::Pid(pid), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT)
    }

    #[cfg(not(target_os = "linux"))]
    fn wait_without_reaping_if_held(pid: Pid) -> nix::Result<WaitStatus> {
        waitpid(pid, None)
    }

    /// 子の終了まで待つ（ブロッキング）。Linux では回収しないまま終了を検知する（`LEADER_HELD`）。
    /// pty の子はセッションリーダーなので、wait は自分で行う
    /// （portable-pty の wait はシグナルの番号を名前に変えてしまうため）。
    pub fn wait_exit(_child: Box<dyn Child + Send + Sync>, pid: u32) -> ExitInfo {
        let unknown = ExitInfo {
            exit_code: None,
            signal: None,
        };
        let Some(pid) = pgid(pid) else { return unknown };
        loop {
            match wait_without_reaping_if_held(pid) {
                Ok(WaitStatus::Exited(_, code)) => {
                    return ExitInfo {
                        exit_code: Some(code),
                        signal: None,
                    };
                }
                Ok(WaitStatus::Signaled(_, signal, _)) => {
                    return ExitInfo {
                        exit_code: None,
                        signal: Some(signal as i32),
                    };
                }
                Ok(_) | Err(Errno::EINTR) => {}
                Err(_) => return unknown,
            }
        }
    }

    /// pty の子はセッションリーダー（pgid = pid）なので、プロセスグループ全体へ送る。
    /// グループへ送れなければ（既に無い）pid 単体を試し、それも無ければ何もしない。
    fn signal_group(pid: u32, signal: Signal) {
        let Some(pid) = pgid(pid) else { return };
        if killpg(pid, signal).is_err() {
            let _ = nix::sys::signal::kill(pid, signal);
        }
    }

    pub fn hangup_tree(pid: u32) {
        signal_group(pid, Signal::SIGHUP);
    }

    pub fn kill_tree(pid: u32) {
        signal_group(pid, Signal::SIGKILL);
    }

    /// 回収されずに残っているリーダーを回収する（`LEADER_HELD` のとき。終了済みであること）。
    /// EINTR は再試行し、既に回収済み（ECHILD）は成功として扱う。それ以外の失敗は呼び出し側に返す。
    pub fn reap(pid: u32) -> std::io::Result<()> {
        let Some(pid) = pgid(pid).filter(|_| LEADER_HELD) else {
            return Ok(());
        };
        loop {
            match waitpid(pid, None) {
                Ok(_) | Err(Errno::ECHILD) => return Ok(()),
                Err(Errno::EINTR) => {}
                Err(errno) => return Err(std::io::Error::from(errno)),
            }
        }
    }

    /// グループにまだ生きているプロセスがいるか。ゾンビのリーダー自身は数えない。
    /// /proc を読めなかったときは `Unknown`（いない、とは言わない）。
    #[cfg(target_os = "linux")]
    pub fn group_state(pid: u32) -> GroupState {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return GroupState::Unknown;
        };
        let stats = entries.map(|entry| {
            entry.map(|e| {
                let is_numeric = e
                    .file_name()
                    .to_string_lossy()
                    .bytes()
                    .all(|b| b.is_ascii_digit());
                is_numeric.then(|| std::fs::read_to_string(e.path().join("stat")))
            })
        });
        fold_members(stats, pid)
    }

    /// /proc の各エントリ（数字でないものは `None`）の `stat` から、グループの状態を決める。
    /// 走査の途中で消えたプロセス（NotFound）は無視し、それ以外の失敗や読めない内容があれば、
    /// 生きているメンバーが見つからなかったとしても `Empty` とは言わず `Unknown` にする。
    #[cfg(target_os = "linux")]
    fn fold_members(
        stats: impl Iterator<Item = std::io::Result<Option<std::io::Result<String>>>>,
        pgrp: u32,
    ) -> GroupState {
        let mut unknown = false;
        for stat in stats {
            match stat {
                Ok(None) => {}
                Ok(Some(Ok(text))) => match is_live_member(&text, pgrp) {
                    Some(true) => return GroupState::Alive,
                    Some(false) => {}
                    None => unknown = true,
                },
                Ok(Some(Err(e))) if e.kind() == std::io::ErrorKind::NotFound => {}
                Ok(Some(Err(_))) | Err(_) => unknown = true,
            }
        }
        if unknown {
            GroupState::Unknown
        } else {
            GroupState::Empty
        }
    }

    /// `/proc/<pid>/stat`（`pid (comm) state ppid pgrp ...`）が、pgrp が `pgrp` の、ゾンビでないプロセスか。
    /// comm は `)` を含みうるので、最後の `)` から後ろを読む。読めない内容は None。
    #[cfg(target_os = "linux")]
    fn is_live_member(stat: &str, pgrp: u32) -> Option<bool> {
        let (_, rest) = stat.rsplit_once(')')?;
        let mut fields = rest.split_whitespace();
        let state = fields.next()?;
        let group = fields.nth(1)?.parse::<u32>().ok()?;
        Some(group == pgrp && !matches!(state, "Z" | "X"))
    }

    /// シグナル 0 の送信で調べる。EPERM は「いる」。
    #[cfg(not(target_os = "linux"))]
    pub fn group_state(pid: u32) -> GroupState {
        let Some(pid) = pgid(pid) else {
            return GroupState::Unknown;
        };
        match killpg(pid, None) {
            Ok(()) | Err(Errno::EPERM) => GroupState::Alive,
            Err(Errno::ESRCH) => GroupState::Empty,
            Err(_) => GroupState::Unknown,
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        use std::io::{Error, ErrorKind};

        use super::{GroupState, fold_members, is_live_member};

        #[test]
        fn proc_stat_parsing_ignores_zombies_and_other_groups() {
            assert_eq!(is_live_member("12 (sleep) S 1 12 12 0 -1", 12), Some(true));
            assert_eq!(
                is_live_member("12 (sleep) Z 1 12 12 0 -1", 12),
                Some(false),
                "zombie"
            );
            assert_eq!(
                is_live_member("13 (sleep) S 1 99 12 0 -1", 12),
                Some(false),
                "other group"
            );
            assert_eq!(
                is_live_member("14 (a) b) R 1 12 12 0 -1", 12),
                Some(true),
                "comm with a parenthesis"
            );
            assert_eq!(is_live_member("garbage", 12), None);
        }

        fn stat(text: &str) -> std::io::Result<Option<std::io::Result<String>>> {
            Ok(Some(Ok(text.to_string())))
        }

        #[test]
        fn the_group_state_distinguishes_alive_empty_and_unknown() {
            let zombie = "12 (sh) Z 1 12 12 0 -1";
            let live = "13 (sleep) S 1 12 12 0 -1";
            assert_eq!(fold_members([stat(zombie)].into_iter(), 12), GroupState::Empty);
            assert_eq!(
                fold_members([stat(zombie), stat(live)].into_iter(), 12),
                GroupState::Alive
            );
            // 走査中に消えたプロセスは無視する。
            let vanished = Ok(Some(Err(Error::from(ErrorKind::NotFound))));
            assert_eq!(
                fold_members([vanished, Ok(None)].into_iter(), 12),
                GroupState::Empty
            );
            // 読めなかったものがあって、生きているメンバーも見つからないなら、いないとは言わない。
            let denied = Ok(Some(Err(Error::from(ErrorKind::PermissionDenied))));
            assert_eq!(
                fold_members([stat(zombie), denied].into_iter(), 12),
                GroupState::Unknown
            );
            assert_eq!(
                fold_members([stat("garbage")].into_iter(), 12),
                GroupState::Unknown
            );
            assert_eq!(
                fold_members([Err(Error::other("readdir"))].into_iter(), 12),
                GroupState::Unknown
            );
            // 生きているメンバーが見つかっていれば、読めないものがあっても Alive。
            let denied = Ok(Some(Err(Error::from(ErrorKind::PermissionDenied))));
            assert_eq!(
                fold_members([denied, stat(live)].into_iter(), 12),
                GroupState::Alive
            );
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::process::{Command, Stdio};

    use super::{Child, ExitInfo, GroupState};

    pub fn wait_exit(mut child: Box<dyn Child + Send + Sync>, _pid: u32) -> ExitInfo {
        match child.wait() {
            Ok(status) => ExitInfo {
                exit_code: i32::try_from(status.exit_code()).ok(),
                signal: None,
            },
            Err(_) => ExitInfo {
                exit_code: None,
                signal: None,
            },
        }
    }

    /// Windows にはシグナルが無い。プロセスツリーごと強制終了する（Job Object による封じ込めは後続の段）。
    fn taskkill_tree(pid: u32) {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    pub fn hangup_tree(pid: u32) {
        taskkill_tree(pid);
    }

    pub fn kill_tree(pid: u32) {
        taskkill_tree(pid);
    }

    pub const LEADER_HELD: bool = false;

    pub fn reap(_pid: u32) -> std::io::Result<()> {
        Ok(())
    }

    /// taskkill /T が木ごと止めるので、リーダーの終了後に追いかけて待つものは無い。
    pub fn group_state(_pid: u32) -> GroupState {
        GroupState::Empty
    }
}

pub use platform::{LEADER_HELD, group_state, hangup_tree, kill_tree, reap, wait_exit};

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
        let pid = process.pid;
        assert_eq!(group_state(pid), GroupState::Alive);
        drop(ChildGuard(Some(process.child)));
        // 回収済み（zombie でもない）なので、グループごと消えている。
        assert_eq!(group_state(pid), GroupState::Empty);
    }

    #[test]
    fn a_disarmed_guard_leaves_the_child_running() {
        let cmd = vec!["sleep".to_string(), "300".to_string()];
        let process = spawn(&request(&cmd)).unwrap();
        let pid = process.pid;
        let child = ChildGuard(Some(process.child)).disarm();
        assert!(child.is_some() && group_state(pid) == GroupState::Alive);
        kill_tree(pid);
        let _ = wait_exit(child.unwrap(), pid);
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

#[cfg(all(test, target_os = "linux"))]
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
        let exit = wait_exit(process.child, process.pid);
        assert_eq!(exit.exit_code, Some(0));
        assert!(reap(process.pid).is_ok());
        assert!(
            reap(process.pid).is_ok(),
            "ECHILD after the first reap is success"
        );
    }
}
