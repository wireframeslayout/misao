//! デーモンへの接続（Node 版の `connect.ts`）。

use std::future::Future;
use std::path::Path;

use crate::client::{Client, ClientError};
use crate::error::{CliError, ErrorKind};

impl From<ClientError> for CliError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Rpc { message, .. } => Self::new(ErrorKind::Rpc, message),
            other => Self::new(ErrorKind::Runtime, other.to_string()),
        }
    }
}

/// デーモンへ接続する。つながらなければ daemon_unreachable（misao serve での起動を案内）。
pub async fn connect_daemon(socket: &Path) -> Result<Client, CliError> {
    match Client::connect(socket).await {
        Ok(client) => Ok(client),
        Err(ClientError::Connection(_)) => Err(CliError::new(
            ErrorKind::DaemonUnreachable,
            format!(
                "デーモンに接続できません（{}）。misao serve で起動してください",
                socket.display()
            ),
        )),
        Err(other) => Err(other.into()),
    }
}

/// 接続して f を実行し、どう終わっても接続を閉じる。
pub async fn with_daemon<T, F, Fut>(socket: &Path, f: F) -> Result<T, CliError>
where
    F: FnOnce(Client) -> Fut,
    Fut: Future<Output = Result<T, CliError>>,
{
    let client = connect_daemon(socket).await?;
    let result = f(client.clone()).await;
    client.close();
    result
}
