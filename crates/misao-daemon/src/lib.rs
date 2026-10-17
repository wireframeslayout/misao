#![allow(dead_code)] // 段階導入の途中（次のコミット以降で使われる）。最後のコミットで外す。
//! misao デーモンの本体。接続口（Unix ソケット）、作業場所 / 窓 / ペイン、PTY、出力の記録、イベントを持つ。
//! 画面・タイトル・稼働判定・状態の保存・attach は後続の段。

mod ansi;
mod child_env;
mod connection;
mod core;
mod event_log;
mod ids;
mod layout;
mod layout_handlers;
mod limits;
mod log;
mod pane;
mod pane_handlers;
mod params;
mod pty;
mod registry;
mod ring;
mod rpc;
mod rpc_error;
mod size_arbiter;
mod stream;

pub use connection::serve_connection;
pub use core::{Core, CoreOptions};
pub use limits::Limits;
pub use log::{LogLevel, Logger};

#[cfg(unix)]
mod daemon;
#[cfg(unix)]
mod error;
#[cfg(unix)]
mod pid_file;
#[cfg(unix)]
mod socket;

#[cfg(unix)]
pub use daemon::{Daemon, DaemonOptions};
#[cfg(unix)]
pub use error::DaemonError;
#[cfg(unix)]
pub use pid_file::acquire_pid_file;
#[cfg(unix)]
pub use socket::{ensure_socket_dir, listen_unix_socket, lock_socket_start, remove_stale_socket};
