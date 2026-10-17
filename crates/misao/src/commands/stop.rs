use std::time::Duration;

use serde_json::json;

use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::connect::with_daemon;
use crate::error::CliError;

pub const USAGE: &str = "misao stop [--json]";

/// 停止を要求してから、デーモンのプロセスが終わるのを待つ上限（ペインを閉じる猶予を含む）。
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(unix)]
mod sys {
    use std::time::{Duration, Instant};

    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::CliError;

    /// SIGTERM を送る。デーモンはペインを閉じて終わる。
    pub fn request_stop(_endpoint: &str, pid: u32) -> Result<(), CliError> {
        kill(to_pid(pid)?, Signal::SIGTERM)
            .map_err(|e| CliError::runtime(&format!("デーモンに SIGTERM を送れません: {e}")))
    }

    fn to_pid(pid: u32) -> Result<Pid, CliError> {
        i32::try_from(pid)
            .ok()
            .filter(|&p| p > 1)
            .map(Pid::from_raw)
            .ok_or_else(|| CliError::runtime(&format!("デーモンの pid が正しくありません: {pid}")))
    }

    /// プロセスが無くなるまで待つ。
    pub async fn wait_exit(pid: u32, timeout: Duration) -> Result<bool, CliError> {
        let pid = to_pid(pid)?;
        let deadline = Instant::now() + timeout;
        loop {
            match kill(pid, None) {
                Err(Errno::ESRCH) => return Ok(true),
                _ if Instant::now() >= deadline => return Ok(false),
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::time::Duration;

    use super::CliError;
    use crate::error::ErrorKind;

    /// 停止イベントを立てる。デーモンはペインを閉じて終わる（Windows にはシグナルが無い）。
    pub fn request_stop(endpoint: &str, _pid: u32) -> Result<(), CliError> {
        match misao_daemon::request_stop(endpoint) {
            Ok(true) => Ok(()),
            Ok(false) => Err(CliError::new(
                ErrorKind::Runtime,
                "デーモンの停止の窓口が見つかりません（別のユーザーのデーモン、または misao stop に対応しない版です）",
            )),
            Err(e) => Err(CliError::runtime(&format!("デーモンに停止を要求できません: {e}"))),
        }
    }

    pub async fn wait_exit(pid: u32, timeout: Duration) -> Result<bool, CliError> {
        tokio::task::spawn_blocking(move || misao_daemon::wait_process_exit(pid, timeout))
            .await
            .map_err(|e| CliError::runtime(&e))?
            .map_err(|e| CliError::runtime(&format!("デーモンの終了を待てません: {e}")))
    }
}

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    expect_positionals(&args, 0, 0, USAGE)?;
    let endpoint = socket.to_string_lossy().into_owned();
    let pid = with_daemon(&socket, |client| async move {
        let info = client.request("server.info", json!({})).await?;
        info.get("pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| CliError::runtime(&"invalid result for server.info"))
    })
    .await?;
    sys::request_stop(&endpoint, pid)?;
    if !sys::wait_exit(pid, EXIT_TIMEOUT).await? {
        return Err(CliError::runtime(&format!(
            "デーモン（pid {pid}）が {} 秒以内に終了しませんでした",
            EXIT_TIMEOUT.as_secs()
        )));
    }
    if is_json {
        write_json(io, &json!({"stopped": true, "pid": pid, "socket": endpoint}));
    } else {
        io.out_line(&format!("[misao] 停止しました（pid {pid}）"));
    }
    Ok(0)
}
