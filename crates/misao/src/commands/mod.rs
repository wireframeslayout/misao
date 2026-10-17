//! サブコマンド。各コマンドは終了コードを返し、失敗は `CliError` で伝える。

mod attach;
mod events;
mod kill;
mod label;
mod ls;
mod new;
mod schema;
mod screen;
mod send;
mod shared;
mod status;
mod tail;

use std::path::PathBuf;

use crate::args::{OptSpec, ParsedArgs, flag, string, strings};
use crate::config::Config;
use crate::error::CliError;
use crate::io::CliIo;

pub struct CommandContext<'a> {
    pub args: ParsedArgs,
    pub io: &'a CliIo,
    pub is_json: bool,
    pub config: &'a Config,
    /// 解決済みのソケットパス。
    pub socket: PathBuf,
}

pub struct CommandSpec {
    pub name: &'static str,
    /// 一覧用の 1 行説明（日本語）。
    pub summary: &'static str,
    pub usage: &'static str,
    pub options: &'static [OptSpec],
}

pub const SERVE_USAGE: &str = "misao serve [--socket PATH] [--data DIR]";

pub const COMMANDS: [CommandSpec; 12] = [
    CommandSpec {
        name: "ls",
        summary: "ペインの一覧を表示する（blocked が先頭）",
        usage: ls::USAGE,
        options: &[string("state"), string("task"), string("workspace")],
    },
    CommandSpec {
        name: "attach",
        summary: "ペインへ入る（prefix + d で抜ける）",
        usage: attach::USAGE,
        options: &[flag("readonly"), flag("no-replay")],
    },
    CommandSpec {
        name: "new",
        summary: "新しいペインを作る（cmd 省略時はログインシェル）",
        usage: new::USAGE,
        options: &[
            string("cwd"),
            strings("label"),
            strings("env"),
            string("workspace"),
            string("window"),
            flag("attach"),
        ],
    },
    CommandSpec {
        name: "kill",
        summary: "ペインを閉じる（--window / --workspace で所属ごと閉じる）",
        usage: kill::USAGE,
        options: &[flag("force"), flag("window"), flag("workspace")],
    },
    CommandSpec {
        name: "status",
        summary: "デーモンの稼働状況を表示する",
        usage: status::USAGE,
        options: &[],
    },
    CommandSpec {
        name: "send",
        summary: "ペインへテキストやキーを送る",
        usage: send::USAGE,
        options: &[flag("enter"), string("keys"), flag("stdin")],
    },
    CommandSpec {
        name: "screen",
        summary: "ペインの現在の画面テキストを表示する",
        usage: screen::USAGE,
        options: &[string("lines")],
    },
    CommandSpec {
        name: "tail",
        summary: "ペインの出力を行単位で追い続ける（Ctrl-C で終了）",
        usage: tail::USAGE,
        options: &[string("since"), string("epoch")],
    },
    CommandSpec {
        name: "events",
        summary: "デーモンのイベントを追い続ける（Ctrl-C で終了）",
        usage: events::USAGE,
        options: &[string("since"), string("epoch"), string("pane")],
    },
    CommandSpec {
        name: "label",
        summary: "ペインのラベルを設定・削除し、変更後のラベルを表示する",
        usage: label::USAGE,
        options: &[strings("unset")],
    },
    CommandSpec {
        name: "schema",
        summary: "プロトコルの JSON Schema (server.schema) をそのまま出力する",
        usage: schema::USAGE,
        options: &[],
    },
    CommandSpec {
        name: "serve",
        summary: "デーモンを前面で起動する（SIGTERM / SIGINT で終了）",
        usage: SERVE_USAGE,
        options: &[string("socket"), string("data")],
    },
];

pub fn find_command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// `serve` 以外のコマンドを実行する。
pub async fn run_command(name: &str, ctx: CommandContext<'_>) -> Result<i32, CliError> {
    match name {
        "ls" => ls::run(ctx).await,
        "attach" => attach::run(ctx).await,
        "new" => new::run(ctx).await,
        "kill" => kill::run(ctx).await,
        "status" => status::run(ctx).await,
        "send" => send::run(ctx).await,
        "screen" => screen::run(ctx).await,
        "tail" => tail::run(ctx).await,
        "events" => events::run(ctx).await,
        "label" => label::run(ctx).await,
        "schema" => schema::run(ctx).await,
        other => Err(CliError::usage(format!("不明なコマンドです: {other}"))),
    }
}
