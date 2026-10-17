use serde_json::json;

use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::connect::with_daemon;
use crate::error::CliError;

pub const USAGE: &str = "misao schema";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext { args, io, socket, .. } = ctx;
    expect_positionals(&args, 0, 0, USAGE)?;
    let schema = with_daemon(&socket, |client| async move {
        Ok(client.request("server.schema", json!({})).await?)
    })
    .await?;
    write_json(io, &schema);
    Ok(0)
}
