//! misao: ヘッドレスのペインサーバー（デーモン + CLI）の単一バイナリ。

mod args;
mod attach;
mod cli;
mod client;
mod commands;
mod config;
mod connect;
mod error;
mod follow;
mod io;
mod keys;
mod layout_target;
mod model;
mod open_pane;
mod prompt;
mod send_keys;
mod serve;
mod socket_path;
mod target;
mod tty_sys;
mod view;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let io = io::real_io();
    ExitCode::from(cli::run(&argv, &io).await)
}
