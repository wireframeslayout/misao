//! 接続口への接続。Unix は Unix ソケット、Windows は名前付きパイプ。

use std::path::Path;

#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;

#[cfg(unix)]
pub async fn connect_stream(path: &Path) -> std::io::Result<Stream> {
    tokio::net::UnixStream::connect(path).await
}

#[cfg(windows)]
pub type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// 名前付きパイプの全インスタンスが使用中（`ERROR_PIPE_BUSY`）。空くまで少し待って開き直す。
#[cfg(windows)]
const ERROR_PIPE_BUSY: i32 = 231;
#[cfg(windows)]
const PIPE_BUSY_RETRIES: u32 = 40;
#[cfg(windows)]
const PIPE_BUSY_WAIT: std::time::Duration = std::time::Duration::from_millis(50);

#[cfg(windows)]
pub async fn connect_stream(path: &Path) -> std::io::Result<Stream> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let mut attempts = 0;
    loop {
        match ClientOptions::new().open(path) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < PIPE_BUSY_RETRIES => {
                attempts += 1;
                tokio::time::sleep(PIPE_BUSY_WAIT).await;
            }
            other => return other,
        }
    }
}
