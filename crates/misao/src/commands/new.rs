use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::args::parse_key_values;
use crate::attach::loop_::{AttachLoopOptions, run_attach_loop};
use crate::attach::tty::require_terminal;
use crate::connect::with_daemon;
use crate::error::{CliError, ErrorKind};
use crate::layout_target::resolve_window_for_open;
use crate::open_pane::{OpenPaneRequest, list_panes, login_shell, open_terminal_pane};
use crate::view::pane_view::{foreground_command, short_pane_ids, short_suffix, tildify};

pub const USAGE: &str = "misao new [--cwd DIR] [--label k=v ...] [--env K=V ...] [--workspace W] [--window NAME] [--attach | --json] [-- cmd ...]";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        config,
        socket,
    } = ctx;
    expect_positionals(&args, 0, 0, USAGE)?;
    let is_attach = args.flag("attach");
    if is_attach && is_json {
        return Err(CliError::usage("--attach と --json は同時に指定できません"));
    }
    if is_attach {
        require_terminal(io)?; // 端末でなければ、ペインを作る前に使い方の誤りとして終える
    }
    let cmd = if args.rest.is_empty() {
        login_shell(io.shell.as_deref(), cfg!(windows))?
    } else {
        args.rest.clone()
    };
    let labels = parse_key_values(&args.strings("label"), "--label")?;
    let env = parse_key_values(&args.strings("env"), "--env")?;
    let cwd = crate::socket_path::normalize_lexical(&io.cwd.join(args.string("cwd").unwrap_or(".")));
    let keys = config.keys;
    with_daemon(&socket, |client| async move {
        let window_id =
            resolve_window_for_open(&client, args.string("workspace"), args.string("window")).await?;
        let request = OpenPaneRequest {
            cmd,
            cwd,
            labels,
            env,
            window_id,
        };
        let pane_id = open_terminal_pane(&client, &request).await?;
        let panes = list_panes(&client).await?;
        let created = panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .cloned()
            .ok_or_else(|| {
                CliError::new(
                    ErrorKind::Runtime,
                    format!("作成したペインが一覧にありません: {pane_id}"),
                )
            })?;
        if is_attach {
            let options = AttachLoopOptions {
                client: client.clone(),
                keys,
                initial: created,
                is_readonly: false,
                replay: crate::attach::session::Replay::Snapshot,
            };
            return run_attach_loop(io, options).await;
        }
        if is_json {
            write_json(io, &created.raw);
            return Ok(0);
        }
        let short_id = short_pane_ids(&panes).get(&pane_id).cloned().unwrap_or_default();
        io.out_line(&format!(
            "[misao] ペイン {short_id} を作成しました（{} · {}）",
            foreground_command(&created),
            tildify(&created.cwd, &io.home_dir)
        ));
        io.out_line("       AZITO の Objects に「未登録」として表示されます。登録すると窓として扱えます");
        io.out_line(&format!("       入る: misao attach {}", short_suffix(&short_id)));
        Ok(0)
    })
    .await
}
