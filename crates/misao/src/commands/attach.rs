use super::CommandContext;
use super::shared::{expect_positionals, resolve_pane};
use crate::attach::loop_::{AttachLoopOptions, run_attach_loop};
use crate::attach::session::Replay;
use crate::attach::tty::require_terminal;
use crate::connect::with_daemon;
use crate::error::CliError;

pub const USAGE: &str = "misao attach <target> [--readonly] [--no-replay]";

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        config,
        socket,
        ..
    } = ctx;
    let positionals = expect_positionals(&args, 1, 1, USAGE)?;
    require_terminal(io)?; // 端末でなければ、接続の前に使い方の誤りとして終える
    let (keys, is_readonly) = (config.keys, args.flag("readonly"));
    let replay = if args.flag("no-replay") {
        Replay::None
    } else {
        Replay::Snapshot
    };
    let home = io.home_dir.clone();
    with_daemon(&socket, |client| async move {
        let initial = resolve_pane(&client, &positionals[0], &home).await?;
        run_attach_loop(
            io,
            AttachLoopOptions {
                client: client.clone(),
                keys,
                initial,
                is_readonly,
                replay,
            },
        )
        .await
    })
    .await
}
