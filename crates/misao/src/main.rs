//! misao: ヘッドレスのペインサーバー（デーモン + CLI）の単一バイナリ。

mod cli;
mod config;
mod error;
mod serve;
mod socket_path;

use std::process::ExitCode;

use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    ExitCode::from(cli::run(cli).await)
}
