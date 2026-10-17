use crate::args::ParsedArgs;
use crate::client::Client;
use crate::error::CliError;
use crate::model::PaneInfo;
use crate::open_pane::list_panes;
use crate::target::resolve_target;

use serde_json::Value;

use crate::io::CliIo;

/// 位置引数の個数を検証する。足りない・多いときは使い方を添えた usage エラー。
pub fn expect_positionals(
    args: &ParsedArgs,
    min: usize,
    max: usize,
    usage: &str,
) -> Result<Vec<String>, CliError> {
    if args.positionals.len() < min || args.positionals.len() > max {
        return Err(CliError::usage(format!("使い方: {usage}")));
    }
    Ok(args.positionals.clone())
}

/// 全ペインを取得して、対象指定から 1 つに決める。
pub async fn resolve_pane(client: &Client, query: &str, home_dir: &str) -> Result<PaneInfo, CliError> {
    let panes = list_panes(client).await?;
    resolve_target(query, &panes, home_dir)
}

/// 1 つの JSON 文書として stdout に出す（2 スペースの整形）。
pub fn write_json(io: &CliIo, value: &Value) {
    io.out_line(&serde_json::to_string_pretty(value).unwrap_or_default());
}

/// NDJSON の 1 行として stdout に出す。
pub fn write_json_line(io: &CliIo, value: &Value) {
    io.out_line(&value.to_string());
}
