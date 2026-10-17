//! 接続口（Unix ソケット）への接続。Windows の名前付きパイプは #43。

use std::path::Path;

#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;

#[cfg(unix)]
pub async fn connect_stream(path: &Path) -> std::io::Result<Stream> {
    tokio::net::UnixStream::connect(path).await
}

/// Unix 以外では接続できない（空のストリームを型として用意するだけ）。
#[cfg(not(unix))]
pub type Stream = tokio::io::DuplexStream;

#[cfg(not(unix))]
pub async fn connect_stream(_path: &Path) -> std::io::Result<Stream> {
    Err(std::io::Error::other("この環境ではまだ対応していません"))
}
