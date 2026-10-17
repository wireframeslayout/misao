use serde_json::{Map, Value, json};

use super::CommandContext;
use super::shared::{expect_positionals, resolve_pane, write_json};
use crate::args::parse_key_values;
use crate::connect::with_daemon;
use crate::error::CliError;

pub const USAGE: &str = "misao label <target> [k=v ...] [--unset k ...] [--json]";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    let positionals = expect_positionals(&args, 1, usize::MAX, USAGE)?;
    let (target, pairs) = positionals
        .split_first()
        .ok_or_else(|| CliError::usage(format!("使い方: {USAGE}")))?;
    let unset = args.strings("unset");
    if pairs.is_empty() && unset.is_empty() {
        return Err(CliError::usage(format!("使い方: {USAGE}")));
    }
    let set: Map<String, Value> = parse_key_values(pairs, "ラベル")?
        .into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();
    let (target, home) = (target.clone(), io.home_dir.clone());
    let result = with_daemon(&socket, |client| async move {
        let pane = resolve_pane(&client, &target, &home).await?;
        Ok(client
            .request(
                "pane.set_label",
                json!({"paneId": pane.pane_id, "set": set, "unset": unset}),
            )
            .await?)
    })
    .await?;
    let labels = result.get("labels").cloned().unwrap_or_else(|| json!({}));
    if is_json {
        write_json(io, &json!({"labels": labels}));
    } else if let Some(map) = labels.as_object() {
        for (k, v) in map {
            io.out_line(&format!("{k}={}", v.as_str().unwrap_or_default()));
        }
    }
    Ok(0)
}
