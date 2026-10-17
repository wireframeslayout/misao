//! `tail` / `events` の追従（Node 版の `follow.ts`）。

use serde_json::Value;

use crate::client::{Client, ClientEvent, GapReason, StreamId, StreamPos};
use crate::error::{CliError, ErrorKind};
use crate::io::{CliIo, CliSignal};

pub struct FollowOptions {
    /// 最初の購読で、保持範囲外（truncated）の gap を警告するか。--since を指定したときだけ true。
    pub warn_truncated: bool,
    /// 購読を始める時点のデーモンの epoch（server.info）。
    pub initial_epoch: String,
    pub stream: StreamId,
    pub start: Option<StreamPos>,
}

/// --since / --epoch から購読の開始位置を決める。--epoch は --since と組でだけ使える。
/// --since だけなら現在の epoch の seq とみなし、その旨を stderr に警告する。
pub fn resolve_start_position(
    io: &CliIo,
    since: Option<u64>,
    epoch: Option<&str>,
    current_epoch: &str,
) -> Result<Option<StreamPos>, CliError> {
    let Some(since) = since else {
        if epoch.is_some() {
            return Err(CliError::usage("--epoch は --since と組で指定してください"));
        }
        return Ok(None);
    };
    if let Some(epoch) = epoch {
        return Ok(Some(StreamPos {
            seq: since,
            epoch: epoch.to_string(),
        }));
    }
    io.err_line(&format!(
        "[misao] 警告: --epoch が無いため、--since {since} を現在の epoch ({current_epoch}) の seq として扱います"
    ));
    Ok(Some(StreamPos {
        seq: since,
        epoch: current_epoch.to_string(),
    }))
}

/// ストリームを SIGINT まで追い続ける（終了コード 0）。切断は再接続と since 追従に任せ、
/// 再接続の経過と取りこぼし（gap）は stderr に出す。購読が拒否された・再接続を打ち切ったときは失敗にする。
/// `on_item` は届いた通知の params と、その時点の epoch を受け取る。
pub async fn follow(
    io: &CliIo,
    client: &Client,
    options: FollowOptions,
    mut on_item: impl FnMut(&Value, &str),
) -> Result<i32, CliError> {
    let mut interrupts = io.signals.listen(&[CliSignal::Interrupt]);
    let mut events = client.events();
    let warn = |text: &str| io.err_line(&format!("[misao] {text}"));
    let epoch_of = || client.epoch().unwrap_or_else(|| options.initial_epoch.clone());
    // 再接続したあとの since は seq が連続している前提なので、欠けたら必ず知らせる。
    let mut warn_truncated = options.warn_truncated;
    match &options.stream {
        StreamId::Events => client.subscribe_events(options.start.clone()).await?,
        StreamId::Lines(pane_id) => client.subscribe_lines(pane_id, options.start.clone()).await?,
    }
    let outcome = loop {
        tokio::select! {
            _ = interrupts.recv() => break Ok(0),
            event = events.recv() => {
                let Some(event) = event else { break Ok(0) };
                match event {
                    ClientEvent::Reconnecting { attempt, .. } => {
                        warn_truncated = true;
                        warn(&format!("接続が切れました。再接続しています（{attempt} 回目）"));
                    }
                    ClientEvent::Connected => warn("再接続しました"),
                    ClientEvent::Closed { cause } => {
                        // 再接続先が非互換か、再接続を諦めた。どちらも追い続けられない。
                        let reason = cause.map_or(String::new(), |c| format!(": {c}"));
                        break Err(CliError::new(ErrorKind::Runtime, format!("再接続を打ち切りました{reason}")));
                    }
                    ClientEvent::Gap { reason: GapReason::Epoch, .. } => warn(&format!(
                        "警告: epoch が指定したもの・前回と違うため（デーモンの再起動）、履歴を最初から読み直しました（epoch {}）",
                        epoch_of()
                    )),
                    ClientEvent::Gap { reason: GapReason::Truncated, .. } if warn_truncated => warn(&format!(
                        "警告: 取りこぼしがあります（epoch {}。指定した位置が保持範囲より古いため、残っている分から表示します）",
                        epoch_of()
                    )),
                    ClientEvent::Gap { .. } | ClientEvent::Notification { .. } => {}
                    ClientEvent::SubscriptionError { message, .. } => {
                        break Err(CliError::new(ErrorKind::Rpc, format!("購読が終了しました: {message}")));
                    }
                    ClientEvent::Stream { params, .. } => on_item(&params, &epoch_of()),
                }
            }
        }
    };
    client.unsubscribe(options.stream);
    outcome
}
