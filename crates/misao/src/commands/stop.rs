use std::time::Duration;

use serde_json::json;

use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::client::Client;
use crate::connect::connect_daemon;
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

    use super::{CliError, Client, Daemon, daemon_of};

    /// SIGTERM を送る。デーモンはペインを閉じて終わる。pid は `server.info` を取った接続の相手のものなので、
    /// 送る直前に同じ接続がまだ同じデーモン（pid と epoch が同じ）につながっていることを確かめる
    /// （その間に別のデーモンに入れ替わって pid が再利用された場合に、無関係なプロセスへ送らない）。
    pub async fn request_stop(_endpoint: &str, client: &Client, daemon: &Daemon) -> Result<(), CliError> {
        let again = daemon_of(client).await?;
        if again != *daemon {
            return Err(CliError::runtime(
                &"デーモンが入れ替わったので停止しません。もう一度実行してください",
            ));
        }
        kill(to_pid(daemon.pid)?, Signal::SIGTERM)
            .map_err(|e| CliError::runtime(&format!("デーモンに SIGTERM を送れません: {e}")))
    }

    fn to_pid(pid: u32) -> Result<Pid, CliError> {
        i32::try_from(pid)
            .ok()
            .filter(|&p| p > 1)
            .map(Pid::from_raw)
            .ok_or_else(|| CliError::runtime(&format!("デーモンの pid が正しくありません: {pid}")))
    }

    /// デーモンが片付けを終えるのを待つ。ペインを閉じ終えたデーモンは、最後にソケットを消して終わる。
    /// デーモンの親が回収しないとゾンビのままプロセスが残るので、プロセスの有無だけでは判定しない。
    pub async fn wait_exit(endpoint: &str, pid: u32, timeout: Duration) -> Result<bool, CliError> {
        let pid = to_pid(pid)?;
        let deadline = Instant::now() + timeout;
        loop {
            if !std::path::Path::new(endpoint).exists() || kill(pid, None) == Err(Errno::ESRCH) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::time::Duration;

    use super::{CliError, Client, Daemon};
    use crate::error::ErrorKind;

    /// 停止イベントを立てる。デーモンはペインを閉じて終わる（Windows にはシグナルが無い）。
    /// イベント名は epoch を含むので、`server.info` を取ったデーモン以外には作用しない。
    pub async fn request_stop(endpoint: &str, _client: &Client, daemon: &Daemon) -> Result<(), CliError> {
        match misao_daemon::request_stop(endpoint, &daemon.epoch) {
            Ok(true) => Ok(()),
            Ok(false) => Err(CliError::new(
                ErrorKind::Runtime,
                "デーモンの停止の窓口が見つかりません（別のユーザーのデーモン、または misao stop に対応しない版です）",
            )),
            Err(e) => Err(CliError::runtime(&format!("デーモンに停止を要求できません: {e}"))),
        }
    }

    pub async fn wait_exit(_endpoint: &str, pid: u32, timeout: Duration) -> Result<bool, CliError> {
        tokio::task::spawn_blocking(move || misao_daemon::wait_process_exit(pid, timeout))
            .await
            .map_err(|e| CliError::runtime(&e))?
            .map_err(|e| CliError::runtime(&format!("デーモンの終了を待てません: {e}")))
    }
}

/// `server.info` が教えるデーモンの身元。
#[derive(Debug, PartialEq, Eq)]
pub struct Daemon {
    pub pid: u32,
    pub epoch: String,
}

async fn daemon_of(client: &Client) -> Result<Daemon, CliError> {
    let info = client.request("server.info", json!({})).await?;
    let pid = info
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok());
    let epoch = info.get("epoch").and_then(serde_json::Value::as_str);
    match (pid, epoch) {
        (Some(pid), Some(epoch)) => Ok(Daemon {
            pid,
            epoch: epoch.to_string(),
        }),
        _ => Err(CliError::runtime(&"invalid result for server.info")),
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
    // server.info を取った接続は、停止を要求し終えるまで開いたままにする。
    let client = connect_daemon(&socket).await?;
    let requested = async {
        let daemon = daemon_of(&client).await?;
        sys::request_stop(&endpoint, &client, &daemon).await?;
        Ok::<_, CliError>(daemon)
    }
    .await;
    client.close();
    let Daemon { pid, .. } = requested?;
    if !sys::wait_exit(&endpoint, pid, EXIT_TIMEOUT).await? {
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
