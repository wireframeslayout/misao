//! misao デーモンの本体。接続口（Unix ソケット）、作業場所 / 窓 / ペイン、PTY、出力の記録、イベントを持つ。
//! 画面・タイトル・稼働判定（プロファイルは規則のデータ）も持つ。状態の保存・attach は後続の段。

mod ansi;
mod byte_activity;
mod child_env;
mod connection;
mod core;
mod event_log;
mod foreground;
mod ids;
mod layout;
mod layout_handlers;
mod limits;
mod log;
mod pane;
mod pane_handlers;
mod pane_observer;
mod pane_view;
mod params;
mod profile;
mod pty;
mod registry;
mod ring;
mod rpc;
mod rpc_error;
mod screen;
mod screen_buffer;
mod screen_charset;
mod screen_cursor;
mod screen_edit;
mod screen_emulator;
mod screen_line;
mod screen_osc;
mod screen_perform;
mod screen_reflow;
mod size_arbiter;
mod state_tracker;
mod stream;
mod title_activity;
mod unicode_width;
mod unicode_width_table;

pub use connection::serve_connection;
pub use core::{Core, CoreOptions};
pub use limits::Limits;
pub use log::{LogLevel, Logger};
pub use profile::{Profile, builtin_profiles};

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

#[cfg(test)]
mod node_diff_tests;
