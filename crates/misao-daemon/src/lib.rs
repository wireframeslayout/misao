//! misao デーモンの本体。第 1 段は接続口（Unix ソケット）と `server.info` / `server.schema` だけを持つ。

mod log;

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
