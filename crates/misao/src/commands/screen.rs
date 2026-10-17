use serde_json::Value;

use super::CommandContext;
use super::shared::{expect_positionals, resolve_pane, write_json};
use crate::connect::with_daemon;
use crate::error::CliError;

pub const USAGE: &str = "misao screen <target> [--lines N] [--json]";

/// 画面の末尾の空行を除いたうえで、末尾 lines 行だけを残す。
fn tail_lines(text: &str, lines: Option<u64>) -> String {
    let mut all: Vec<&str> = text.split('\n').collect();
    while all.last().is_some_and(|l| l.trim().is_empty()) {
        all.pop();
    }
    let keep = lines.map_or(all.len(), |n| {
        usize::try_from(n).unwrap_or(usize::MAX).min(all.len())
    });
    all[all.len() - keep..].join("\n")
}

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    let positionals = expect_positionals(&args, 1, 1, USAGE)?;
    let lines = args.int("lines", 1)?;
    let home = io.home_dir.clone();
    let mut screen = with_daemon(&socket, |client| async move {
        let pane = resolve_pane(&client, &positionals[0], &home).await?;
        Ok(client
            .request("pane.screen", serde_json::json!({"paneId": pane.pane_id}))
            .await?)
    })
    .await?;
    let text = tail_lines(
        screen.get("text").and_then(Value::as_str).unwrap_or_default(),
        lines,
    );
    if is_json {
        screen["text"] = Value::String(text);
        write_json(io, &screen);
    } else {
        io.out_line(&text);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::tail_lines;

    #[test]
    fn trailing_blank_rows_are_dropped_before_taking_the_tail() {
        assert_eq!(tail_lines("a\nb\nc\n\n  \n", None), "a\nb\nc");
        assert_eq!(tail_lines("a\nb\nc\n\n", Some(2)), "b\nc");
        assert_eq!(tail_lines("a\n\n", Some(5)), "a");
        assert_eq!(tail_lines("\n\n", None), "");
    }
}
