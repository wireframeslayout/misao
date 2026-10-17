use serde_json::{Value, json};

use super::CommandContext;
use super::shared::{expect_positionals, resolve_pane, write_json_line};
use crate::client::StreamId;
use crate::connect::with_daemon;
use crate::error::CliError;
use crate::follow::{FollowOptions, follow, resolve_start_position};
use crate::io::CliIo;

pub const USAGE: &str = "misao tail <target> [--since SEQ [--epoch EPOCH]] [--json]";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    let positionals = expect_positionals(&args, 1, 1, USAGE)?;
    let since = args.int("since", 0)?;
    let epoch_arg = args.string("epoch").map(str::to_string);
    let home = io.home_dir.clone();
    with_daemon(&socket, |client| async move {
        let pane = resolve_pane(&client, &positionals[0], &home).await?;
        let info = client.request("server.info", json!({})).await?;
        let epoch = info
            .get("epoch")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // 指定が無ければ保持している分を最初から再生してから追う。
        let start = resolve_start_position(io, since, epoch_arg.as_deref(), &epoch)?.unwrap_or(
            crate::client::StreamPos {
                seq: 0,
                epoch: epoch.clone(),
            },
        );
        let options = FollowOptions {
            warn_truncated: since.is_some(),
            initial_epoch: epoch,
            stream: StreamId::Lines(pane.pane_id),
            start: Some(start),
        };
        follow(io, &client, options, |line, epoch| {
            print_line(io, is_json, line, epoch)
        })
        .await
    })
    .await
}

fn print_line(io: &CliIo, is_json: bool, line: &Value, epoch: &str) {
    if is_json {
        let mut out = line.clone();
        out["epoch"] = json!(epoch);
        write_json_line(io, &out);
    } else {
        io.out_line(line.get("text").and_then(Value::as_str).unwrap_or_default());
    }
}
