use serde_json::json;

use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::error::{CliError, ErrorKind};
use crate::open_pane::list_panes;
use crate::prompt::confirm;
use crate::target::resolve_target;
use crate::view::pane_view::display_name;

pub const USAGE: &str = "misao kill <target> [--window | --workspace] [--force] [--json]";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Pane,
    Window,
    Workspace,
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Window => "window",
            Self::Workspace => "workspace",
        }
    }
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
    let target = &positionals[0];
    let scope = if args.flag("window") {
        Scope::Window
    } else if args.flag("workspace") {
        Scope::Workspace
    } else {
        Scope::Pane
    };
    if args.flag("window") && args.flag("workspace") {
        return Err(CliError::usage("--window と --workspace は同時に指定できません"));
    }
    let is_forced = args.flag("force");
    if !is_forced && !io.stdin.is_tty {
        return Err(CliError::usage(
            "確認できないため閉じません。端末から実行するか --force を付けてください",
        ));
    }
    let client = crate::connect::connect_daemon(&socket).await?;
    let result = async {
        let panes = list_panes(&client).await?;
        let pane = resolve_target(target, &panes, &io.home_dir)?;
        let in_window: Vec<_> = panes.iter().filter(|p| p.window.id == pane.window.id).collect();
        let in_workspace: Vec<_> = panes.iter().filter(|p| p.workspace == pane.workspace).collect();
        let label = match scope {
            Scope::Pane => display_name(&pane, &io.home_dir),
            Scope::Window => format!("窓「{}」（ペイン {} 個）", pane.window.name, in_window.len()),
            Scope::Workspace => format!(
                "workspace「{}」（ペイン {} 個）",
                pane.workspace,
                in_workspace.len()
            ),
        };
        if !is_forced && !confirm(io, &format!("[misao] {label} を閉じます。よろしいですか [y/N] ")).await
        {
            return Err(CliError::new(ErrorKind::Aborted, "中止しました"));
        }
        match scope {
            Scope::Pane => {
                client
                    .request("pane.close", json!({"paneId": pane.pane_id}))
                    .await?
            }
            Scope::Window => {
                client
                    .request("window.close", json!({"windowId": pane.window.id}))
                    .await?
            }
            Scope::Workspace => {
                client
                    .request("workspace.close", json!({"name": pane.workspace}))
                    .await?
            }
        };
        let pane_ids: Vec<&str> = match scope {
            Scope::Pane => vec![pane.pane_id.as_str()],
            Scope::Window => in_window.iter().map(|p| p.pane_id.as_str()).collect(),
            Scope::Workspace => in_workspace.iter().map(|p| p.pane_id.as_str()).collect(),
        };
        if is_json {
            write_json(io, &json!({"closed": scope.name(), "paneIds": pane_ids}));
        } else {
            io.out_line(&format!("[misao] {label} を閉じました"));
        }
        Ok(0)
    }
    .await;
    client.close();
    result
}
