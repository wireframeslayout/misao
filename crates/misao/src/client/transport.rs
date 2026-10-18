//! 接続口への接続。Unix は Unix ソケット、Windows は名前付きパイプ。

use std::path::Path;

/// サーバーの身元を確かめられなかったときのエラーの印（`connect_daemon` が理由をそのまま利用者に見せる）。
pub const SERVER_IDENTITY_MARK: &str = "refusing to connect, the pipe server is not trusted";

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
            // 接続できたら、サーバーが自分と同じユーザーのものか確かめる。別のユーザーが先に同じ名前で待ち受けている
            // （名前の先取り）なら、そこへ何も送らずにやめる。
            Ok(client) => {
                use std::os::windows::io::AsRawHandle;
                return match misao_daemon::check_pipe_server(client.as_raw_handle()) {
                    Ok(()) => Ok(client),
                    Err(e) => Err(std::io::Error::new(
                        e.kind(),
                        format!("{SERVER_IDENTITY_MARK}: {e}"),
                    )),
                };
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < PIPE_BUSY_RETRIES => {
                attempts += 1;
                tokio::time::sleep(PIPE_BUSY_WAIT).await;
            }
            other => return other,
        }
    }
}
