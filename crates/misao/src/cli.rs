//! 引数の解釈とコマンドの振り分け（Node 版の `run.ts`）。`process::exit` はエントリ（main）だけが呼ぶ。

use std::path::PathBuf;

use crate::args::{SplitArgs, parse_command_args, split_command};
use crate::commands::{COMMANDS, CommandContext, SERVE_USAGE, find_command, run_command};
use crate::config::{LoadInput, load_config, resolve_socket};
use crate::error::{CliError, ErrorKind};
use crate::io::CliIo;
use crate::serve::{ServeArgs, serve};
use crate::socket_path::PathEnv;
use crate::target::TARGET_HELP;

/// この CLI / デーモンのビルド版。
pub const MISAO_VERSION: &str = env!("CARGO_PKG_VERSION");
const PACKAGE: &str = "misao";

const GLOBAL_HELP: [&str; 4] = [
    "--config PATH  設定ファイル (misao.json) を指定する",
    "--json         出力を JSON にする",
    "--help         このヘルプを表示する",
    "--version      バージョンを表示する",
];

/// 環境変数・ホーム・カレントディレクトリ。プロセスとの境界で一度だけ読む。
pub struct Env {
    pub path: PathEnv,
    pub config: Option<String>,
    pub home: String,
    pub cwd: PathBuf,
}

impl Env {
    pub fn from_io(io: &CliIo) -> Result<Self, CliError> {
        if io.home_dir.is_empty() {
            return Err(CliError::new(
                ErrorKind::Runtime,
                "cannot determine the home directory",
            ));
        }
        let var = |name: &str| io.env.get(name).cloned();
        Ok(Self {
            path: PathEnv {
                socket: var("MISAO_SOCKET"),
                dir: var("MISAO_DIR"),
            },
            config: var("MISAO_CONFIG"),
            home: io.home_dir.clone(),
            cwd: io.cwd.clone(),
        })
    }
}

fn help_text() -> String {
    let width = COMMANDS.iter().map(|c| c.name.chars().count()).max().unwrap_or(0);
    let mut lines = vec![
        "使い方: misao [オプション] <コマンド> [引数]".to_string(),
        String::new(),
        "コマンド:".to_string(),
    ];
    lines.extend(
        COMMANDS
            .iter()
            .map(|c| format!("  {:<width$}  {}", c.name, c.summary)),
    );
    lines.push(String::new());
    lines.push("オプション:".to_string());
    lines.extend(GLOBAL_HELP.iter().map(|l| format!("  {l}")));
    lines.push(String::new());
    lines.extend(TARGET_HELP.iter().map(|l| (*l).to_string()));
    lines.join("\n")
}

fn command_help_text(usage: &str, summary: &str) -> String {
    let mut lines = vec![format!("使い方: {usage}"), summary.to_string()];
    if usage.contains("<target>") {
        lines.push(String::new());
        lines.extend(TARGET_HELP.iter().map(|l| (*l).to_string()));
    }
    lines.join("\n")
}

/// argv を解釈してコマンドを実行し、終了コードを返す。
pub async fn run(argv: &[String], io: &CliIo) -> u8 {
    let SplitArgs {
        command,
        args,
        globals,
    } = split_command(argv);
    let is_json = globals.flag("json");
    if globals.flag("version") {
        io.out_line(&format!("{PACKAGE} {MISAO_VERSION}"));
        return 0;
    }
    if globals.flag("help") && command.as_deref().and_then(find_command).is_none() {
        io.out_line(&help_text());
        return 0;
    }
    match execute(command, &args, io, is_json).await {
        Ok(code) => u8::try_from(code).unwrap_or(1),
        Err(error) => {
            io.err_line(&error.render(is_json));
            error.exit_code()
        }
    }
}

async fn execute(
    command: Option<String>,
    args: &[String],
    io: &CliIo,
    is_json: bool,
) -> Result<i32, CliError> {
    let name =
        command.ok_or_else(|| CliError::usage(format!("コマンドを指定してください\n{}", help_text())))?;
    let spec = find_command(&name)
        .ok_or_else(|| CliError::usage(format!("不明なコマンドです: {name}\n{}", help_text())))?;
    let parsed = parse_command_args(args, spec.options)?;
    if parsed.flag("help") {
        io.out_line(&command_help_text(spec.usage, spec.summary));
        return Ok(0);
    }
    let env = Env::from_io(io)?;
    let flag_path = parsed.string("config").map(PathBuf::from);
    let loaded = load_config(&LoadInput {
        flag_path: flag_path.as_deref(),
        env_config: env.config.as_deref(),
        env: &env.path,
        home: &env.home,
    })?;
    for warning in &loaded.warnings {
        io.err_line(&format!("[misao] 警告: {warning}"));
    }
    let socket = resolve_socket(&loaded, &env.path, &env.home)?;
    if name == "serve" {
        if !parsed.positionals.is_empty() {
            return Err(CliError::usage(format!("使い方: {SERVE_USAGE}")));
        }
        let serve_args = ServeArgs {
            socket: parsed.string("socket").map(str::to_string),
            data: parsed.string("data").map(str::to_string),
        };
        serve(&env, &loaded.config, serve_args).await?;
        return Ok(0);
    }
    let ctx = CommandContext {
        args: parsed,
        io,
        is_json,
        config: &loaded.config,
        socket,
    };
    run_command(&name, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_help_lists_every_command_and_the_target_syntax() {
        let help = help_text();
        for c in &COMMANDS {
            assert!(
                help.lines().any(|l| l.starts_with(&format!("  {} ", c.name))),
                "{}",
                c.name
            );
        }
        assert!(help.lines().any(|l| l == "対象 <target> の書き方:"));
        assert!(help.contains("数字だけの ID は p_ を付けて書く"));
    }

    #[test]
    fn only_commands_that_take_a_target_show_the_target_syntax() {
        assert!(command_help_text("misao send <target> [text]", "x").contains("対象 <target> の書き方:"));
        assert!(!command_help_text("misao status [--json]", "x").contains("対象 <target>"));
    }
}
