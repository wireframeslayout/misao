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
    pub cols: u32,
    pub rows: u32,
}

pub struct PtyProcess {
    pub pid: u32,
    pub master: Box<dyn MasterPty + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

/// 端末のサイズ。pty の寸法は 16 bit なので、超える値は上限に丸める。
pub fn pty_size(cols: u32, rows: u32) -> PtySize {
    let clamp = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
    PtySize {
        rows: clamp(rows),
        cols: clamp(cols),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// 子プロセスを擬似端末で起動する。失敗の文字列は `pane.open` の InvalidParams（`spawn failed: ...`）に使う。
pub fn spawn(req: &SpawnRequest<'_>) -> Result<PtyProcess, String> {
    let (program, args) = req.cmd.split_first().ok_or("cmd is empty")?;
    // portable-pty は cwd がディレクトリでないとき黙ってホームに切り替えるので、先に確かめる。
    if !Path::new(req.cwd).is_dir() {
        return Err(format!("cwd is not a directory: {}", req.cwd));
    }
    let pair = native_pty_system()
        .openpty(pty_size(req.cols, req.rows))
        .map_err(|e| format!("{e:#}"))?;
    let mut builder = CommandBuilder::new(program);
    builder.args(args);
    builder.cwd(req.cwd);
    builder.env_clear();
    for (key, value) in &req.env {
        builder.env(key, value);
    }
    let child = pair.slave.spawn_command(builder).map_err(|e| format!("{e:#}"))?;
    // slave を親が持ち続けると、子が終わっても master が EOF にならない。
    drop(pair.slave);
    let pid = child.process_id().ok_or("the child has no process id")?;
    let reader = pair.master.try_clone_reader().map_err(|e| format!("{e:#}"))?;
    let writer = pair.master.take_writer().map_err(|e| format!("{e:#}"))?;
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

    use super::{Child, ExitInfo};

    fn pgid(pid: u32) -> Option<Pid> {
        i32::try_from(pid).ok().map(Pid::from_raw)
    }

    /// 子の終了まで待つ（ブロッキング）。pty の子はセッションリーダーなので、wait は自分で行う
    /// （portable-pty の wait はシグナルの番号を名前に変えてしまうため）。
    pub fn wait_exit(_child: Box<dyn Child + Send + Sync>, pid: u32) -> ExitInfo {
        let unknown = ExitInfo {
            exit_code: None,
            signal: None,
        };
        let Some(pid) = pgid(pid) else { return unknown };
        loop {
            match waitpid(pid, None) {
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

    /// グループにまだ生きているプロセスがいるか（シグナル 0 の送信）。EPERM は「いる」。
    pub fn tree_alive(pid: u32) -> bool {
        pgid(pid).is_some_and(|pid| !matches!(killpg(pid, None), Err(Errno::ESRCH)))
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

    /// taskkill /T が木ごと止めるので、リーダーの終了後に追いかけて待つものは無い。
    pub fn tree_alive(_pid: u32) -> bool {
        false
    }
}

pub use platform::{hangup_tree, kill_tree, tree_alive, wait_exit};
