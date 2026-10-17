use serde_json::Value;

use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::connect::with_daemon;
use crate::error::CliError;
use crate::model::PaneInfo;
use crate::open_pane::list_panes;
use crate::view::pane_view::{
    display_name, foreground_command, pane_state_key, relative_time, short_pane_ids, sort_panes, state_label,
    task_label, task_number, tildify,
};
use crate::view::table::render_table;

pub const USAGE: &str = "misao ls [--state blocked|working|idle|exited] [--task N] [--workspace W] [--json]";
const STATE_FILTERS: [&str; 4] = ["blocked", "working", "idle", "exited"];
const HEADER: [&str; 7] = ["STATE", "PANE", "NAME", "TASK", "AGENT", "CWD", "LAST"];

/// short_ids は絞り込み前の全ペインから作る（表示した末尾を対象指定に使っても曖昧にならないように）。
fn render_rows(
    panes: &[PaneInfo],
    short_ids: &std::collections::HashMap<String, String>,
    home_dir: &str,
    now: i64,
) -> Vec<String> {
    let mut rows = vec![HEADER.iter().map(|h| (*h).to_string()).collect::<Vec<_>>()];
    rows.extend(panes.iter().map(|p| {
        vec![
            state_label(p),
            short_ids.get(&p.pane_id).cloned().unwrap_or_default(),
            display_name(p, home_dir),
            task_label(p),
            foreground_command(p),
            tildify(&p.cwd, home_dir),
            relative_time(p.last_output_at.as_deref(), now),
        ]
    }));
    render_table(&rows)
}

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    expect_positionals(&args, 0, 0, USAGE)?;
    let state = args.string("state");
    if let Some(state) = state
        && !STATE_FILTERS.contains(&state)
    {
        return Err(CliError::usage(format!(
            "--state は {} のいずれかを指定してください: {state}",
            STATE_FILTERS.join(" / ")
        )));
    }
    let task = args
        .string("task")
        .map(|t| t.strip_prefix('#').unwrap_or(t).to_string());
    let workspace = args.string("workspace");
    let all = with_daemon(&socket, |client| async move { list_panes(&client).await }).await?;
    let panes: Vec<PaneInfo> = sort_panes(&all)
        .into_iter()
        .filter(|p| {
            state.is_none_or(|s| pane_state_key(p) == s)
                && task.as_ref().is_none_or(|t| task_number(p).as_ref() == Some(t))
                && workspace.is_none_or(|w| p.workspace == w)
        })
        .collect();
    if is_json {
        write_json(io, &Value::Array(panes.iter().map(|p| p.raw.clone()).collect()));
    } else if panes.is_empty() {
        io.out_line("[misao] 該当するペインはありません");
    } else {
        for line in render_rows(&panes, &short_pane_ids(&all), &io.home_dir, io.now_ms()) {
            io.out_line(&line);
        }
    }
    Ok(0)
}
