use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use super::CommandContext;
use super::shared::{expect_positionals, resolve_pane, write_json};
use crate::connect::with_daemon;
use crate::error::CliError;
use crate::send_keys::encode_keys;

pub const USAGE: &str = "misao send <target> [text] [--enter] [--keys Enter,Escape,C-c] [--stdin] [--json]（- で始まる text は -- の後ろに書く）";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    let positionals = expect_positionals(&args, 1, 2, USAGE)?;
    let target = positionals[0].clone();
    let positional_text = positionals.get(1);
    if positional_text.is_some() && !args.rest.is_empty() {
        return Err(CliError::usage(format!(
            "text と -- の後ろの text は同時に指定できません。使い方: {USAGE}"
        )));
    }
    let text = positional_text
        .cloned()
        .or_else(|| (!args.rest.is_empty()).then(|| args.rest.join(" ")));
    let keys = args.string("keys");
    let is_enter = args.flag("enter");
    if args.flag("stdin") && text.is_some() {
        return Err(CliError::usage("--stdin と text は同時に指定できません"));
    }
    if text.is_none() && keys.is_none() && !is_enter && !args.flag("stdin") {
        return Err(CliError::usage(format!("送る内容がありません。使い方: {USAGE}")));
    }
    let mut data = if args.flag("stdin") {
        io.stdin.read_all().await
    } else {
        text.unwrap_or_default().into_bytes()
    };
    if let Some(keys) = keys {
        let names: Vec<&str> = keys.split(',').filter(|k| !k.is_empty()).collect();
        data.extend(encode_keys(&names)?);
    }
    if is_enter {
        data.extend(encode_keys(&["Enter"])?);
    }
    let home = io.home_dir.clone();
    let payload = STANDARD.encode(&data);
    let pane_id = with_daemon(&socket, |client| async move {
        let pane = resolve_pane(&client, &target, &home).await?;
        client
            .request(
                "pane.write",
                json!({"paneId": pane.pane_id, "dataB64": payload, "source": "terminal"}),
            )
            .await?;
        Ok(pane.pane_id)
    })
    .await?;
    if is_json {
        write_json(io, &json!({"ok": true, "paneId": pane_id, "bytes": data.len()}));
    } else {
        io.out_line(&format!("[misao] {pane_id} へ {} バイト送りました", data.len()));
    }
    Ok(0)
}
