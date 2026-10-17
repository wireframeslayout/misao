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
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
    use nix::unistd::Pid;

    use super::{Child, ExitInfo};

    fn pgid(pid: u32) -> Option<Pid> {
        i32::try_from(pid).ok().map(Pid::from_raw)
    }

    /// リーダーを `reap` まで回収せずに（ゾンビのまま）残すか。残せば、リーダーの pid（= pgid = sid）は
    /// 回収されるまで再利用されないので、`pane.close` の時点でグループへ signal しても無関係なグループを止めない。
    /// `waitid(WNOWAIT)` と /proc の走査が使える Linux だけ。他の Unix は終了時に回収する。
    pub const LEADER_HELD: bool = cfg!(target_os = "linux");

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
            let waited = if LEADER_HELD {
                waitid(Id::Pid(pid), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT)
            } else {
                waitpid(pid, None)
            };
            match waited {
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
    pub fn reap(pid: u32) {
        if LEADER_HELD && let Some(pid) = pgid(pid) {
            let _ = waitpid(pid, None);
        }
    }

    /// グループにまだ生きているプロセスがいるか。ゾンビのリーダー自身は数えない。
    #[cfg(target_os = "linux")]
    pub fn tree_alive(pid: u32) -> bool {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        entries.filter_map(Result::ok).any(|entry| {
            let is_numeric = entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit());
            is_numeric
                && std::fs::read_to_string(entry.path().join("stat"))
                    .is_ok_and(|stat| is_live_member(&stat, pid))
        })
    }

    /// `/proc/<pid>/stat`（`pid (comm) state ppid pgrp ...`）が、pgrp が `pgrp` の、ゾンビでないプロセスか。
    /// comm は `)` を含みうるので、最後の `)` から後ろを読む。
    #[cfg(target_os = "linux")]
    fn is_live_member(stat: &str, pgrp: u32) -> bool {
        let Some((_, rest)) = stat.rsplit_once(')') else {
            return false;
        };
        let mut fields = rest.split_whitespace();
        let state = fields.next();
        let group = fields.nth(1).and_then(|f| f.parse::<u32>().ok());
        group == Some(pgrp) && !matches!(state, Some("Z" | "X"))
    }

    /// グループにまだ生きているプロセスがいるか（シグナル 0 の送信）。EPERM は「いる」。
    #[cfg(not(target_os = "linux"))]
    pub fn tree_alive(pid: u32) -> bool {
        pgid(pid).is_some_and(|pid| !matches!(killpg(pid, None), Err(Errno::ESRCH)))
    }

    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        use super::is_live_member;

        #[test]
        fn proc_stat_parsing_ignores_zombies_and_other_groups() {
            assert!(is_live_member("12 (sleep) S 1 12 12 0 -1", 12));
            assert!(!is_live_member("12 (sleep) Z 1 12 12 0 -1", 12), "zombie");
            assert!(!is_live_member("13 (sleep) S 1 99 12 0 -1", 12), "other group");
            assert!(
                is_live_member("14 (a) b) R 1 12 12 0 -1", 12),
                "comm containing a parenthesis"
            );
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::process::{Command, Stdio};

    use super::{Child, ExitInfo};

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

    pub fn reap(_pid: u32) {}

    /// taskkill /T が木ごと止めるので、リーダーの終了後に追いかけて待つものは無い。
    pub fn tree_alive(_pid: u32) -> bool {
        false
    }
}

pub use platform::{LEADER_HELD, hangup_tree, kill_tree, reap, tree_alive, wait_exit};

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
        assert!(tree_alive(pid));
        drop(ChildGuard(Some(process.child)));
        // 回収済み（zombie でもない）なので、グループごと消えている。
        assert!(!tree_alive(pid));
    }

    #[test]
    fn a_disarmed_guard_leaves_the_child_running() {
        let cmd = vec!["sleep".to_string(), "300".to_string()];
        let process = spawn(&request(&cmd)).unwrap();
        let pid = process.pid;
        let child = ChildGuard(Some(process.child)).disarm();
        assert!(child.is_some() && tree_alive(pid));
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
