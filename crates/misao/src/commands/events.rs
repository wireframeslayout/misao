use serde_json::{Value, json};

use super::CommandContext;
use super::shared::{expect_positionals, resolve_pane, write_json_line};
use crate::client::StreamId;
use crate::connect::with_daemon;
use crate::error::CliError;
use crate::follow::{FollowOptions, follow, resolve_start_position};

pub const USAGE: &str = "misao events [--since SEQ [--epoch EPOCH]] [--pane <target>] [--json]";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    expect_positionals(&args, 0, 0, USAGE)?;
    let since = args.int("since", 0)?;
    let epoch_arg = args.string("epoch").map(str::to_string);
    let pane_query = args.string("pane").map(str::to_string);
    let home = io.home_dir.clone();
    with_daemon(&socket, |client| async move {
        let pane_id = match &pane_query {
            Some(query) => Some(resolve_pane(&client, query, &home).await?.pane_id),
            None => None,
        };
        let info = client.request("server.info", json!({})).await?;
        let epoch = info
            .get("epoch")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // 指定が無ければライブのみ。
        let start = resolve_start_position(io, since, epoch_arg.as_deref(), &epoch)?;
        let options = FollowOptions {
            warn_truncated: since.is_some(),
            initial_epoch: epoch,
            stream: StreamId::Events,
            start,
        };
        follow(io, &client, options, |event, epoch| {
            if let Some(want) = &pane_id
                && event.get("paneId").and_then(Value::as_str) != Some(want.as_str())
            {
                return;
            }
            if is_json {
                let mut out = event.clone();
                out["epoch"] = json!(epoch);
                write_json_line(io, &out);
            } else {
                io.out_line(&format!(
                    "{}  {}  {}  {}  {}",
                    event.get("seq").map_or(String::new(), Value::to_string),
                    event.get("ts").and_then(Value::as_str).unwrap_or_default(),
                    event.get("type").and_then(Value::as_str).unwrap_or_default(),
                    event.get("paneId").and_then(Value::as_str).unwrap_or("-"),
                    event.get("data").map_or("null".to_string(), Value::to_string),
                ));
            }
        })
        .await
    })
    .await
}
