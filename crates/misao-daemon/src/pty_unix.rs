//! Unix（Linux / macOS）のプロセスツリー: pty の子はセッションリーダー（pid = pgid = sid）なので、
//! プロセスグループへ signal を送って止める。
//!
//! リーダーは `pane.close` / shutdown まで回収しない（`waitid(WNOWAIT)`）。回収するまでリーダーの pid は
//! 再利用されないので、close の時点でグループへ signal しても無関係なグループを止めない。
//! Linux は nix の `waitid` と /proc の走査、macOS は libc の `waitid` と `ps` を使う。
#![cfg_attr(not(target_os = "linux"), allow(unsafe_code))]

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::waitpid;
use nix::unistd::Pid;

use super::{Child, ExitInfo, GroupState};

/// リーダーを `reap` まで回収せずに（ゾンビのまま）残すか。Linux と macOS は `waitid(WNOWAIT)` で残せる。
pub const LEADER_HELD: bool = true;

/// pty の子（セッションリーダー）のプロセスツリー。
#[derive(Debug, Clone)]
pub struct ProcessTree {
    pid: u32,
}

fn pgid(pid: u32) -> Option<Pid> {
    i32::try_from(pid).ok().map(Pid::from_raw)
}

impl ProcessTree {
    pub fn attach(_child: &(dyn Child + Send + Sync), pid: u32) -> Result<Self, String> {
        Ok(Self { pid })
    }

    /// 子の終了まで待つ（ブロッキング）。回収はしない（`LEADER_HELD`）。
    /// pty の子はセッションリーダーなので、wait は自分で行う
    /// （portable-pty の wait はシグナルの番号を名前に変えてしまうため）。
    pub fn wait_exit(&self, _child: Box<dyn Child + Send + Sync>) -> ExitInfo {
        let unknown = ExitInfo {
            exit_code: None,
            signal: None,
        };
        let Some(pid) = pgid(self.pid) else { return unknown };
        loop {
            match wait_without_reaping(pid) {
                Ok(Some(info)) => return info,
                Ok(None) | Err(Errno::EINTR) => {}
                Err(_) => return unknown,
            }
        }
    }

    /// pty の子はセッションリーダー（pgid = pid）なので、プロセスグループ全体へ送る。
    /// グループへ送れなければ（既に無い）pid 単体を試し、それも無ければ何もしない。
    fn signal_group(&self, signal: Signal) {
        let Some(pid) = pgid(self.pid) else { return };
        if killpg(pid, signal).is_err() {
            let _ = nix::sys::signal::kill(pid, signal);
        }
    }

    pub fn hangup(&self) {
        self.signal_group(Signal::SIGHUP);
    }

    pub fn kill(&self) {
        self.signal_group(Signal::SIGKILL);
    }

    /// 回収されずに残っているリーダーを回収する（終了済みであること）。
    /// EINTR は再試行し、既に回収済み（ECHILD）は成功として扱う。それ以外の失敗は呼び出し側に返す。
    pub fn reap(&self) -> std::io::Result<()> {
        let Some(pid) = pgid(self.pid) else {
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
    pub fn group_state(&self) -> GroupState {
        group_state(self.pid)
    }
}

/// 終了を待つ。回収はしない。終了以外の状態変化（`None`）は呼び出し側が待ち直す。
#[cfg(target_os = "linux")]
fn wait_without_reaping(pid: Pid) -> nix::Result<Option<ExitInfo>> {
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    Ok(
        match waitid(Id::Pid(pid), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT)? {
            WaitStatus::Exited(_, code) => Some(ExitInfo {
                exit_code: Some(code),
                signal: None,
            }),
            WaitStatus::Signaled(_, signal, _) => Some(ExitInfo {
                exit_code: None,
                signal: Some(signal as i32),
            }),
            _ => None,
        },
    )
}

/// macOS には nix の `waitid` が無いので libc を直接呼ぶ（`WNOWAIT` は macOS の `waitid(2)` にある）。
#[cfg(not(target_os = "linux"))]
fn wait_without_reaping(pid: Pid) -> nix::Result<Option<ExitInfo>> {
    use nix::libc;

    // SAFETY: 全ビットがゼロの siginfo_t は有効な値で、waitid が成功したときだけ中身を読む。
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: `info` は書き込める siginfo_t で、この呼び出しの間だけ借りる。pid は自分の子。
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            libc::id_t::try_from(pid.as_raw()).map_err(|_| Errno::EINVAL)?,
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(Errno::last());
    }
    Ok(match info.si_code {
        libc::CLD_EXITED => Some(ExitInfo {
            exit_code: Some(info.si_status),
            signal: None,
        }),
        libc::CLD_KILLED | libc::CLD_DUMPED => Some(ExitInfo {
            exit_code: None,
            signal: Some(info.si_status),
        }),
        _ => None,
    })
}

/// グループにまだ生きているプロセスがいるか。ゾンビのリーダー自身は数えない。
/// /proc を読めなかったときは `Unknown`（いない、とは言わない）。
#[cfg(target_os = "linux")]
fn group_state(pid: u32) -> GroupState {
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

/// macOS にはプロセスを列挙する /proc が無いので、`ps` で全プロセスの pgid と状態を一度に読む。
#[cfg(not(target_os = "linux"))]
fn group_state(pid: u32) -> GroupState {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,pgid=,stat="])
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => fold_ps_members(&String::from_utf8_lossy(&out.stdout), pid),
        _ => GroupState::Unknown,
    }
}

/// `ps -A -o pid=,pgid=,stat=` の出力から、pgid が `pgrp` の、ゾンビでないプロセスがいるかを決める。
/// 読めない行があって、生きているメンバーも見つからないなら、いないとは言わず `Unknown`。
#[cfg(any(not(target_os = "linux"), test))]
fn fold_ps_members(output: &str, pgrp: u32) -> GroupState {
    let mut unknown = false;
    for line in output.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let (Some(_pid), Some(group), Some(state)) = (fields.next(), fields.next(), fields.next()) else {
            unknown = true;
            continue;
        };
        match group.parse::<u32>() {
            Ok(group) if group == pgrp && !state.starts_with('Z') => return GroupState::Alive,
            Ok(_) => {}
            Err(_) => unknown = true,
        }
    }
    if unknown {
        GroupState::Unknown
    } else {
        GroupState::Empty
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn ps_output_ignores_zombies_and_other_groups() {
        let out = "  1     1 Ss\n 12    12 Z+\n 13    12 S+\n 14    99 S\n";
        assert_eq!(fold_ps_members(out, 12), GroupState::Alive);
        assert_eq!(
            fold_ps_members(" 12    12 Z\n 14    99 S\n", 12),
            GroupState::Empty
        );
        assert_eq!(fold_ps_members("", 12), GroupState::Empty);
        assert_eq!(fold_ps_members(" 12 Z\n", 12), GroupState::Unknown);
        assert_eq!(fold_ps_members(" 12 x S\n", 12), GroupState::Unknown);
        // 生きているメンバーが見つかっていれば、読めない行があっても Alive。
        assert_eq!(fold_ps_members("garbage\n 13 12 S\n", 12), GroupState::Alive);
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use std::io::{Error, ErrorKind};

        use super::*;

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
