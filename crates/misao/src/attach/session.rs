//! 1 つのペインへ attach して、終わるまで入出力をつなぐ（Node 版の `attach/session.ts`）。

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tokio::sync::mpsc::unbounded_channel;

use super::prefix_filter::{PrefixAction, PrefixFilter};
use super::tty::{TtyGuard, require_terminal};
use crate::client::{Client, ClientError, ClientEvent, StreamId};
use crate::error::CliError;
use crate::io::{CliIo, CliSignal};
use crate::keys::{KeyBindings, format_key_spec};
use crate::model::PaneInfo;
use crate::open_pane::pane_info;
use crate::view::pane_view::{display_name, foreground_command, pane_state_key, task_number};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    Snapshot,
    None,
}

impl Replay {
    fn name(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::None => "none",
        }
    }
}

pub struct SessionOptions<'a> {
    pub client: &'a Client,
    pub io: &'a CliIo,
    pub pane: &'a PaneInfo,
    pub keys: KeyBindings,
    /// 入力を送らず、resize もしない。
    pub is_readonly: bool,
    pub replay: Replay,
}

#[derive(Debug)]
pub enum SessionReason {
    Action(PrefixAction),
    Exited {
        exit_code: Option<i64>,
        signal: Option<i64>,
    },
    Disconnected,
    Signal,
    /// 入力の送信や resize がデーモンに拒否された。
    Failed(CliError),
}

/// `input_count` は、この attach で CLI が送った pane.write の回数。
#[derive(Debug)]
pub struct SessionEnd {
    pub reason: SessionReason,
    pub input_count: u32,
}

/// `[misao] W-806 · misao 計画 · task #419 · claude · blocked · 抜ける: Ctrl-^ d · 次/前: Ctrl-^ n/p · 一覧: Ctrl-^ l`
pub fn format_banner(pane: &PaneInfo, keys: &KeyBindings, is_readonly: bool, home_dir: &str) -> String {
    let prefix = format_key_spec(keys.prefix);
    let mut parts = vec![display_name(pane, home_dir)];
    if let Some(task) = task_number(pane) {
        parts.push(format!("task #{task}"));
    }
    parts.push(foreground_command(pane));
    parts.push(pane_state_key(pane).to_string());
    if is_readonly {
        parts.push("READONLY".to_string());
    }
    parts.push(format!("抜ける: {prefix} {}", format_key_spec(keys.detach)));
    parts.push(format!(
        "次/前: {prefix} {}/{}",
        format_key_spec(keys.next),
        format_key_spec(keys.prev)
    ));
    parts.push(format!("一覧: {prefix} {}", format_key_spec(keys.list)));
    format!("[misao] {}", parts.join(" · "))
}

/// どの終わり方（prefix アクション / ペイン終了 / 接続断 / シグナル / 例外）でも、端末の raw mode を戻して
/// TTY_RESET を書いてから返す。
pub async fn run_session(opts: SessionOptions<'_>) -> Result<SessionEnd, CliError> {
    let SessionOptions {
        client,
        io,
        pane,
        keys,
        is_readonly,
        replay,
    } = opts;
    require_terminal(io)?;
    let size = io.terminal_size();
    let mut guard = TtyGuard::new(io);
    io.err_line(&format_banner(pane, &keys, is_readonly, &io.home_dir));
    guard.enter()?;
    let mut state = SessionState {
        is_attached: false,
        is_disconnected: false,
    };
    let result = drive(client, io, pane, keys, is_readonly, replay, size, &mut state).await;
    client.unsubscribe(StreamId::Events);
    guard.restore();
    if state.is_attached && !state.is_disconnected {
        match client
            .request("pane.detach", json!({"paneId": pane.pane_id}))
            .await
        {
            Ok(_) | Err(ClientError::Connection(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }
    result
}

struct SessionState {
    is_attached: bool,
    is_disconnected: bool,
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    client: &Client,
    io: &CliIo,
    pane: &PaneInfo,
    keys: KeyBindings,
    is_readonly: bool,
    replay: Replay,
    size: Option<(usize, usize)>,
    state: &mut SessionState,
) -> Result<SessionEnd, CliError> {
    let pane_id = pane.pane_id.as_str();
    let client_id = format!("cli-{}", io.pid);
    let mut filter = PrefixFilter::new(keys);
    let mut input_count = 0u32;
    let mut events = client.events();
    let mut signals = io.signals.listen(&[
        CliSignal::Terminate,
        CliSignal::Hangup,
        CliSignal::Interrupt,
        CliSignal::Quit,
        CliSignal::WindowChange,
    ]);
    let (failed_tx, mut failed_rx) = unbounded_channel::<ClientError>();
    let end = |reason: SessionReason, input_count: u32| Ok(SessionEnd { reason, input_count });
    if let Err(e) = setup(client, pane_id, &client_id, replay, is_readonly, size, state).await {
        return match e {
            ClientError::Connection(_) => {
                state.is_disconnected = true;
                end(SessionReason::Disconnected, 0)
            }
            other => Err(other.into()),
        };
    }
    // attach の前に終わっていたペインは終了イベントが来ないので、状態を見て終わらせる。
    let current = pane_info(client, pane_id).await?;
    if current.process_state != "running" {
        return end(
            SessionReason::Exited {
                exit_code: current.exit_code,
                signal: current.signal,
            },
            input_count,
        );
    }
    let reason = loop {
        tokio::select! {
            chunk = io.stdin.recv() => {
                let Some(chunk) = chunk else { break SessionReason::Signal };
                let result = filter.feed(&chunk);
                if !result.forward.is_empty() && !is_readonly {
                    input_count += 1;
                    send_input(client, pane_id, &client_id, &result.forward, &failed_tx);
                }
                if let Some(action) = result.action {
                    break SessionReason::Action(action);
                }
            }
            event = events.recv() => match event {
                None => break SessionReason::Disconnected,
                Some(ClientEvent::Connected) => {}
                Some(ClientEvent::Notification { method, params }) => {
                    if method == "pane.output" && params.get("paneId").and_then(Value::as_str) == Some(pane_id) {
                        write_output(io, &params);
                    }
                }
                Some(ClientEvent::Stream { stream: StreamId::Events, params }) => {
                    if let Some(reason) = exit_reason(&params, pane_id) {
                        break reason;
                    }
                }
                Some(ClientEvent::Reconnecting { .. } | ClientEvent::Closed { .. }) => {
                    state.is_disconnected = true;
                    break SessionReason::Disconnected;
                }
                Some(_) => {}
            },
            signal = signals.recv() => match signal {
                Some(CliSignal::WindowChange) => {
                    if !is_readonly && let Some((cols, rows)) = io.terminal_size() {
                        let params = json!({"paneId": pane_id, "cols": cols, "rows": rows, "clientId": client_id});
                        watch_failure(client.request_nowait("pane.resize", params), &failed_tx);
                    }
                }
                Some(_) => break SessionReason::Signal,
                None => {}
            },
            failure = failed_rx.recv() => {
                if let Some(error) = failure {
                    break SessionReason::Failed(error.into());
                }
            }
        }
    };
    end(reason, input_count)
}

async fn setup(
    client: &Client,
    pane_id: &str,
    client_id: &str,
    replay: Replay,
    is_readonly: bool,
    size: Option<(usize, usize)>,
    state: &mut SessionState,
) -> Result<(), ClientError> {
    client.subscribe_events(None).await?;
    let mut params = json!({"paneId": pane_id, "clientId": client_id, "replay": replay.name()});
    if let (false, Some((cols, rows))) = (is_readonly, size) {
        params["cols"] = json!(cols);
        params["rows"] = json!(rows);
    }
    client.request("pane.attach", params).await?;
    state.is_attached = true;
    Ok(())
}

fn exit_reason(event: &Value, pane_id: &str) -> Option<SessionReason> {
    if event.get("type").and_then(Value::as_str) != Some("pane.exited")
        || event.get("paneId").and_then(Value::as_str) != Some(pane_id)
    {
        return None;
    }
    let data = event.get("data")?;
    Some(SessionReason::Exited {
        exit_code: data.get("exitCode").and_then(Value::as_i64),
        signal: data.get("signal").and_then(Value::as_i64),
    })
}

fn write_output(io: &CliIo, params: &Value) {
    if let Some(data) = params
        .get("dataB64")
        .and_then(Value::as_str)
        .and_then(|b| STANDARD.decode(b).ok())
    {
        io.stdout.write(&data);
    }
}

fn send_input(
    client: &Client,
    pane_id: &str,
    client_id: &str,
    data: &[u8],
    failed: &tokio::sync::mpsc::UnboundedSender<ClientError>,
) {
    let params = json!({
        "paneId": pane_id,
        "dataB64": STANDARD.encode(data),
        "source": "terminal",
        "clientId": client_id,
    });
    watch_failure(client.request_nowait("pane.write", params), failed);
}

/// 送った要求が拒否されたら failed へ知らせる（呼んだ順の送信は request_nowait が保証する）。
fn watch_failure(
    sent: Result<tokio::sync::oneshot::Receiver<Result<Value, ClientError>>, ClientError>,
    failed: &tokio::sync::mpsc::UnboundedSender<ClientError>,
) {
    let failed = failed.clone();
    tokio::spawn(async move {
        let outcome = match sent {
            Ok(answer) => answer
                .await
                .unwrap_or(Err(ClientError::Connection("not connected".into()))),
            Err(e) => Err(e),
        };
        // 接続断は別経路（状態の通知）で扱う。拒否だけを失敗にする。
        if let Err(e @ ClientError::Rpc { .. }) = outcome {
            let _ = failed.send(e);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::make_pane;

    #[test]
    fn the_banner_is_built_from_the_key_settings() {
        let pane = make_pane(|v| {
            v["labels"] = json!({"windowId": "806", "name": "misao 計画", "task": "419"});
            v["agentState"] = json!("blocked");
            v["fgCommand"] = json!("claude");
        });
        assert_eq!(
            format_banner(&pane, &KeyBindings::default(), false, "/home/test"),
            "[misao] W-806 · misao 計画 · task #419 · claude · blocked · 抜ける: Ctrl-^ d · 次/前: Ctrl-^ n/p · 一覧: Ctrl-^ l"
        );
        let custom = KeyBindings {
            prefix: 0x01,
            detach: b'q',
            ..KeyBindings::default()
        };
        let readonly = format_banner(
            &make_pane(|v| v["cmd"] = json!(["/bin/zsh"])),
            &custom,
            true,
            "/home/test",
        );
        assert!(
            readonly
                .starts_with("[misao] (未登録) zsh · ~/work · zsh · idle · READONLY · 抜ける: Ctrl-A q · "),
            "{readonly}"
        );
    }
}
