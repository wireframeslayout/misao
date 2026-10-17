use std::path::{Path, PathBuf};

use crate::cli::Env;
use crate::config::Config;
use crate::error::{CliError, ErrorKind};
use crate::socket_path::{PathEnv, normalize_lexical, resolve_socket_path};

#[cfg(unix)]
const PID_FILE_NAME: &str = "daemon.pid";
#[cfg(unix)]
const STATE_FILE_NAME: &str = "persistence.json";

pub struct ServeArgs {
    pub socket: Option<String>,
    pub data: Option<String>,
}

/// `~` と相対パスを解決する（Node 版の `--socket` / `--data` と同じ: `~` 始まりはホーム、それ以外は cwd 基準）。
fn resolve_user_path(env: &Env, value: &str) -> String {
    if value.starts_with('~') {
        value.to_string()
    } else {
        normalize_lexical(&env.cwd.join(value))
            .to_string_lossy()
            .into_owned()
    }
}

fn resolve_data_dir(env: &Env, dir: &str) -> PathBuf {
    if dir == "~" {
        PathBuf::from(&env.home)
    } else if let Some(rest) = dir.strip_prefix("~/") {
        Path::new(&env.home).join(rest)
    } else {
        normalize_lexical(&env.cwd.join(dir))
    }
}

/// 設定（`misao.json`）と引数から、ソケットパスと、pid ファイル・保存した状態の置き場所（data ディレクトリ）を決める。
pub fn resolve_paths(env: &Env, config: &Config, args: &ServeArgs) -> Result<(PathBuf, PathBuf), CliError> {
    let socket = match &args.socket {
        Some(s) => {
            let explicit = resolve_user_path(env, s);
            resolve_socket_path(&PathEnv::default(), Some(&explicit), &env.home)
                .map_err(|e| CliError::new(ErrorKind::Usage, format!("--socket が正しくありません: {e}")))?
        }
        None => resolve_socket_path(&env.path, config.socket.as_deref(), &env.home)?,
    };
    let data_dir = match &args.data {
        Some(d) => resolve_data_dir(env, d),
        None => socket
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env.cwd.clone()),
    };
    Ok((socket, data_dir))
}

#[cfg(unix)]
pub async fn serve(env: &Env, config: &Config, args: ServeArgs) -> Result<(), CliError> {
    use std::os::unix::fs::DirBuilderExt;

    use misao_daemon::{Daemon, DaemonOptions, Logger, builtin_profiles};
    use tokio::signal::unix::{SignalKind, signal};

    let (socket_path, data_dir) = resolve_paths(env, config, &args)?;
    if args.data.is_some() {
        let dir = data_dir.as_path();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| CliError::runtime(&e))?;
    }
    // 起動中に届いたシグナルも取りこぼさないよう、start の前に登録する。
    let mut int = signal(SignalKind::interrupt()).map_err(|e| CliError::runtime(&e))?;
    let mut term = signal(SignalKind::terminate()).map_err(|e| CliError::runtime(&e))?;
    let mut hup = signal(SignalKind::hangup()).map_err(|e| CliError::runtime(&e))?;
    let shutdown = async {
        tokio::select! {
            _ = int.recv() => {}
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
    };
    let daemon = Daemon::start(DaemonOptions {
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        socket_path,
        state_path: data_dir.join(STATE_FILE_NAME),
        pid_path: data_dir.join(PID_FILE_NAME),
        limits: config.limits,
        log: Logger::stderr(config.log_level),
        profiles: builtin_profiles().map_err(|e| CliError::new(ErrorKind::Runtime, e))?,
    })
    .await
    .map_err(|e| CliError::runtime(&e))?;
    daemon.run(shutdown).await;
    Ok(())
}

#[cfg(not(unix))]
pub async fn serve(env: &Env, config: &Config, args: ServeArgs) -> Result<(), CliError> {
    resolve_paths(env, config, &args)?;
    Err(CliError::new(
        ErrorKind::Runtime,
        "serve is not supported on this platform yet (the Windows connection endpoint is tracked in docs/*/windows.md)",
    ))
}
