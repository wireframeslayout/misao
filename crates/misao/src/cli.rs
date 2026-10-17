use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{LoadInput, load_config};
use crate::error::{CliError, ErrorKind};
use crate::serve::{ServeArgs, serve};
use crate::socket_path::PathEnv;

#[derive(Debug, Parser)]
#[command(
    name = "misao",
    version,
    about = "ヘッドレスのペインサーバー（デーモン + CLI）"
)]
pub struct Cli {
    /// 設定ファイル (misao.json) を指定する
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// 出力を JSON にする
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// デーモンを前面で起動する（SIGTERM / SIGINT で終了）
    Serve(ServeCommand),
}

#[derive(Debug, clap::Args)]
struct ServeCommand {
    /// ソケットのパス（設定より優先）
    #[arg(long, value_name = "PATH")]
    socket: Option<String>,
    /// pid ファイルの置き場所（既定はソケットの親ディレクトリ）
    #[arg(long, value_name = "DIR")]
    data: Option<String>,
}

/// プロセスとの境界。環境変数・ホーム・カレントディレクトリはここで一度だけ読む。
pub struct Env {
    pub path: PathEnv,
    pub config: Option<String>,
    pub home: String,
    pub cwd: PathBuf,
}

fn read_env() -> Result<Env, CliError> {
    let var = |name: &str| std::env::var(name).ok();
    let home = std::env::home_dir()
        .ok_or_else(|| CliError::new(ErrorKind::Runtime, "cannot determine the home directory"))?;
    let cwd = std::env::current_dir().map_err(|e| CliError::runtime(&e))?;
    Ok(Env {
        path: PathEnv {
            socket: var("MISAO_SOCKET"),
            dir: var("MISAO_DIR"),
        },
        config: var("MISAO_CONFIG"),
        home: home.to_string_lossy().into_owned(),
        cwd,
    })
}

/// 引数を実行して終了コードを返す。`process::exit` はエントリだけが呼ぶ。
pub async fn run(cli: Cli) -> u8 {
    let is_json = cli.json;
    match execute(cli).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", e.render(is_json));
            e.exit_code()
        }
    }
}

async fn execute(cli: Cli) -> Result<(), CliError> {
    let env = read_env()?;
    let loaded = load_config(&LoadInput {
        flag_path: cli.config.as_deref(),
        env_config: env.config.as_deref(),
        env: &env.path,
        home: &env.home,
    })?;
    for w in &loaded.warnings {
        eprintln!("[misao] 警告: {w}");
    }
    match cli.command {
        Command::Serve(args) => {
            serve(
                &env,
                &loaded.config,
                ServeArgs {
                    socket: args.socket,
                    data: args.data,
                },
            )
            .await
        }
    }
}
