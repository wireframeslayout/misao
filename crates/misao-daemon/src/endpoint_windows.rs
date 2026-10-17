//! 接続口（名前付きパイプ `\\.\pipe\...`）。起動したユーザーだけが接続できる ACL を付け、
//! リモートからの接続を断り、同じ名前の先取りを防ぐ。
#![allow(unsafe_code)]

use std::io;
use std::path::Path;

use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};

use crate::win_security::UserOnlyAcl;
use crate::windows_dir::ensure_data_dir;
use crate::{DaemonError, Logger};

pub type Connection = NamedPipeServer;

/// Windows の `ERROR_ACCESS_DENIED` / `ERROR_PIPE_BUSY`: 最初のインスタンスとして作れない（同じ名前のパイプが既にある）。
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;

/// 起動の前に、保存した状態の置き場所（`--data`）を用意して検証する。パイプには親ディレクトリが無い。
pub fn prepare(_socket_path: &Path, state_path: &Path) -> Result<(), DaemonError> {
    ensure_data_dir(state_path)
}

pub struct Endpoint {
    name: String,
    acl: UserOnlyAcl,
    /// 接続を待っているインスタンス。接続が来たら、次のインスタンスを作ってから渡す。
    pending: Option<NamedPipeServer>,
}

impl Endpoint {
    /// `FILE_FLAG_FIRST_PIPE_INSTANCE` で作る。同じ名前のパイプが既にあれば（別のデーモン、または名前の先取り）失敗する。
    pub async fn bind(path: &Path, _log: &Logger) -> Result<Self, DaemonError> {
        let name = path.to_string_lossy().into_owned();
        let acl = UserOnlyAcl::new()
            .map_err(|e| DaemonError::io("cannot build the pipe access control list", e))?;
        let first = create_instance(&name, &acl, true).map_err(|e| match e.raw_os_error() {
            Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY) => DaemonError::AlreadyListening(path.to_path_buf()),
            _ => DaemonError::io(format!("cannot listen on {name}"), e),
        })?;
        Ok(Self {
            name,
            acl,
            pending: Some(first),
        })
    }

    /// 次の接続を待つ。接続されたインスタンスを返す前に、次のインスタンスを作っておく
    /// （パイプのインスタンスが無い間は、クライアントが接続できない）。
    pub async fn accept(&mut self) -> io::Result<Connection> {
        let Some(server) = &self.pending else {
            return std::future::pending().await;
        };
        server.connect().await?;
        let next = create_instance(&self.name, &self.acl, false)?;
        self.pending
            .replace(next)
            .ok_or_else(|| io::Error::other("no pending pipe instance"))
    }

    pub fn stop_listening(&mut self) {
        self.pending = None;
    }

    /// パイプは、作ったプロセスが終わればカーネルが消す。消すものは無い。
    pub fn remove(&self) {}
}

fn create_instance(name: &str, acl: &UserOnlyAcl, first: bool) -> io::Result<NamedPipeServer> {
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte);
    // SAFETY: `acl.attributes()` は、有効な SECURITY_ATTRIBUTES（`acl` が生きている間だけ有効）を指す。
    // 呼び出しの間だけ読まれ、パイプは記述子を複製して持つ。
    unsafe { options.create_with_security_attributes_raw(name, acl.attributes()) }
}
