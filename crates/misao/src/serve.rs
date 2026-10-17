use std::path::{Path, PathBuf};

use crate::cli::Env;
use crate::config::Config;
use crate::error::{CliError, ErrorKind};
use crate::socket_path::{PathEnv, normalize_lexical, resolve_socket_path};

const PID_FILE_NAME: &str = "daemon.pid";
const STATE_FILE_NAME: &str = "persistence.json";

#[cfg(unix)]
#[path = "serve_unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "serve_windows.rs"]
mod sys;

pub struct ServeArgs {
    pub socket: Option<String>,
    pub data: Option<String>,
}

/// `~` と相対パスを解決する（Node 版の `--socket` / `--data` と同じ: `~` 始まりはホーム、それ以外は cwd 基準）。
/// Windows の名前付きパイプ（`\\.\pipe\...`）は、パスではないのでそのまま。
fn resolve_user_path(env: &Env, value: &str) -> String {
    if value.starts_with('~') || cfg!(windows) && crate::pipe_name::is_named_pipe_path(value) {
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
        None => default_data_dir(env, &socket)?,
    };
    Ok((socket, data_dir))
}

/// `--data` が無いときの pid ファイル・保存した状態の置き場所。Unix はソケットの親ディレクトリ。
/// 名前付きパイプには親ディレクトリが無いので、misao のディレクトリ（`$MISAO_DIR` > `~/.misao`）にする。
#[cfg(unix)]
fn default_data_dir(env: &Env, socket: &Path) -> Result<PathBuf, CliError> {
    Ok(socket
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| env.cwd.clone()))
}

#[cfg(windows)]
fn default_data_dir(env: &Env, _socket: &Path) -> Result<PathBuf, CliError> {
    crate::pipe_name::windows_misao_dir(env.path.dir.as_deref(), &env.home)
        .map(PathBuf::from)
        .map_err(CliError::from)
}

/// デーモンを前面で起動し、停止の要求（Unix はシグナル、Windows はコンソールの終了と `misao stop`）で終了する。
pub async fn serve(env: &Env, config: &Config, args: ServeArgs) -> Result<(), CliError> {
    use misao_daemon::{Daemon, DaemonOptions, Logger, builtin_profiles};

    let (socket_path, data_dir) = resolve_paths(env, config, &args)?;
    if args.data.is_some() {
        sys::create_data_dir(&data_dir).map_err(|e| CliError::runtime(&e))?;
    }
    // 起動中に届いた停止の要求も取りこぼさないよう、start の前に登録する。
    let shutdown = sys::Shutdown::register().map_err(|e| CliError::runtime(&e))?;
    let endpoint = socket_path.to_string_lossy().into_owned();
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
    match shutdown.wait_future(&endpoint) {
        Ok(wait) => {
            daemon.run(wait).await;
            Ok(())
        }
        Err(e) => {
            // 停止の窓口を用意できなければ、止められないデーモンを残さず、ここで片付ける。
            daemon.run(async {}).await;
            Err(CliError::runtime(&e))
        }
    }
}
