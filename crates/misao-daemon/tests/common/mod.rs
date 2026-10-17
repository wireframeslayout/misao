//! デーモンの中核を、ソケット無しで（メモリ上のストリームで）動かすテスト用クライアント。
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use misao_daemon::{Core, CoreOptions, Limits, LogLevel, Logger, builtin_profiles, serve_connection};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

pub const WAIT: Duration = Duration::from_secs(10);

pub fn core_with(limits: Limits) -> Arc<Core> {
    Core::new(CoreOptions {
        version: Some("test".to_string()),
        socket_path: PathBuf::from("/tmp/misao-test.sock"),
        limits,
        log: Logger::new(LogLevel::Error, |m| eprintln!("{m}")),
        state_path: None,
        profiles: builtin_profiles().unwrap(),
    })
    .unwrap()
}

/// 状態を `state_path` へ保存する（あれば復元する）中核。
pub fn core_at(state_path: &std::path::Path) -> Arc<Core> {
    Core::new(CoreOptions {
        version: Some("test".to_string()),
        socket_path: PathBuf::from("/tmp/misao-test.sock"),
        limits: Limits::default(),
        log: Logger::new(LogLevel::Error, |m| eprintln!("{m}")),
        state_path: Some(state_path.to_path_buf()),
        profiles: builtin_profiles().unwrap(),
    })
    .unwrap()
}

pub fn core() -> Arc<Core> {
    core_with(Limits::default())
}

pub struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
    /// 応答を待つ間に届いた通知。
    pub notifications: Vec<Value>,
}

pub fn connect(core: &Arc<Core>) -> Client {
    connect_with_buffer(core, 1024 * 1024)
}

/// 送受信バッファの小さい接続（読まないクライアントの再現に使う）。
pub fn connect_with_buffer(core: &Arc<Core>, buffer: usize) -> Client {
    let (client, server) = tokio::io::duplex(buffer);
    tokio::spawn(serve_connection(server, Arc::clone(core)));
    let (reader, writer) = tokio::io::split(client);
    Client {
        reader: BufReader::new(reader),
        writer,
        next_id: 1,
        notifications: Vec::new(),
    }
}

impl Client {
    pub async fn send_raw(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
    }

    /// 次の 1 行（応答または通知）。無ければ `WAIT` で失敗する。
    pub async fn read_message(&mut self) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(WAIT, self.reader.read_line(&mut line))
            .await
            .expect("timed out waiting for a message")
            .unwrap();
        assert!(n > 0, "connection closed");
        serde_json::from_str(&line).unwrap()
    }

    /// 次の通知（応答は読み飛ばさない）。
    pub async fn next_notification(&mut self) -> Value {
        if !self.notifications.is_empty() {
            return self.notifications.remove(0);
        }
        let msg = self.read_message().await;
        assert!(msg.get("method").is_some(), "expected a notification: {msg}");
        msg
    }

    /// 応答（成功・エラーとも）を返す。待つ間に届いた通知は `notifications` に溜める。
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send_raw(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string())
            .await;
        loop {
            let msg = self.read_message().await;
            if msg.get("method").is_some() {
                self.notifications.push(msg);
            } else {
                assert_eq!(msg["id"], json!(id), "{msg}");
                return msg;
            }
        }
    }

    /// 成功を期待して result を返す。
    pub async fn ok(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params).await;
        assert!(response.get("error").is_none(), "{method} failed: {response}");
        response["result"].clone()
    }

    /// エラーを期待してコードを返す。
    pub async fn err_code(&mut self, method: &str, params: Value) -> i64 {
        let response = self.call(method, params).await;
        response["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("{method} should fail: {response}"))
    }
}

/// 条件が満たされるまで待つ。`WAIT` で失敗する。
pub async fn wait_until<F, Fut>(what: &str, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + WAIT;
    while !cond().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `sh -c <script>` を pane で動かす cmd。
pub fn sh(script: &str) -> Value {
    json!(["sh", "-c", script])
}

/// ファイルが現れ、中身が空でなくなるまで待って、末尾の改行を除いた中身を返す。
pub async fn read_file_when_ready(path: &std::path::Path) -> String {
    wait_until(&format!("{}", path.display()), || async {
        std::fs::read_to_string(path).is_ok_and(|s| s.ends_with('\n'))
    })
    .await;
    std::fs::read_to_string(path).unwrap().trim_end().to_string()
}

/// pid のプロセスが生きているか（`kill -0`）。
pub fn is_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl Client {
    /// 接続が閉じられるまで読み捨てる。`WAIT` までに閉じなければ失敗する。
    pub async fn drain_until_closed(&mut self) {
        let mut line = String::new();
        loop {
            line.clear();
            let n = tokio::time::timeout(WAIT, self.reader.read_line(&mut line))
                .await
                .expect("the connection was not closed");
            if n.unwrap_or(0) == 0 {
                return;
            }
        }
    }

    /// 通知を `count` 件集める（応答は混ざらない前提）。
    pub async fn notifications(&mut self, count: usize) -> Vec<Value> {
        let mut out = Vec::new();
        while out.len() < count {
            out.push(self.next_notification().await);
        }
        out
    }

    /// 少しの間通知が来ないことを確かめる。
    pub async fn assert_no_notification(&mut self, quiet: Duration) {
        let mut line = String::new();
        if let Ok(read) = tokio::time::timeout(quiet, self.reader.read_line(&mut line)).await {
            panic!("unexpected message ({read:?}): {line}");
        }
    }
}
