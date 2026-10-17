//! pane.* の RPC のうち、状態を変えないもの（info / list）と入出力（write / resize / screen / 行の購読）。

use std::collections::HashSet;
use std::sync::Arc;

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use misao_protocol::ErrorCode;
use serde_json::{Map, Value, json};

use crate::core::{Core, State, to_value};
use crate::foreground::{ForegroundQuery, read_foreground_commands};
use crate::pane::{LinesSource, Pane, RunState};
use crate::params::{
    self, InputSource, PaneListParams, PaneRefParams, PaneResizeParams, PaneWriteParams, SubscribeLinesParams,
};
use crate::registry::{Body, PaneEntry, PaneRegistry};
use crate::rpc::RequestContext;
use crate::rpc_error::{RpcFailure, RpcResult};
use crate::stream;

type Raw<'a> = Option<&'a Map<String, Value>>;

/// 末尾の `=` の有無を問わず読む（Node の `Buffer.from(s, 'base64')` に近づける）。
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

fn ok() -> Value {
    json!({"ok": true})
}

/// 入力の待ち行列が一杯（子が入力を読まない）。待たずに返す。Node 版には対応するエラーが無いので、内部エラーの番号を使う。
fn input_queue_full(queued: usize) -> RpcFailure {
    RpcFailure::internal(format!(
        "pane input queue is full ({queued} bytes pending); the child is not reading its input"
    ))
}

fn pane_exited() -> RpcFailure {
    RpcFailure::new(ErrorCode::PANE_EXITED, "pane has exited")
}

/// `pane.info` を作るのに必要なもの。全体の lock の中で取っておき、lock を離してから info（前面プロセスの取得を含む）を作る。
enum InfoSource {
    Live {
        pane: Arc<Pane>,
        workspace: String,
        window: Value,
        labels: Value,
    },
    /// 再起動後に復元した pane。実行体が無いので、保存してある記録からそのまま作れる。
    Stopped(Value),
}

/// 所属の window が消えていれば WindowNotFound。
fn snapshot(state: &State, entry: &PaneEntry) -> RpcResult<InfoSource> {
    let location = state.layout.window_ref(&entry.window_id)?;
    let window = to_value(&location.window)?;
    let labels = to_value(&entry.labels)?;
    Ok(match &entry.body {
        Body::Live(pane) => InfoSource::Live {
            pane: Arc::clone(pane),
            workspace: location.workspace.clone(),
            window,
            labels,
        },
        Body::Stopped(s) => InfoSource::Stopped(json!({
            "paneId": entry.pane_id,
            "pid": null,
            "cmd": s.cmd,
            "cwd": s.cwd,
            "workspace": location.workspace,
            "window": window,
            "labels": labels,
            "processState": "stopped",
            "exitCode": null,
            "signal": null,
            "agentState": "unknown",
            "decidedBy": "none",
            "title": "",
            "lastOutputAt": null,
            "cols": s.cols,
            "rows": s.rows,
            "clients": [],
            "sizeOwner": null,
        })),
    })
}

/// スナップショットから `pane.info` の結果を作る。前面プロセスは、全 pane ぶんをまとめて取る。
fn render_infos(sources: Vec<InfoSource>) -> Vec<Value> {
    let queries: Vec<(usize, ForegroundQuery)> = sources
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match s {
            InfoSource::Live { pane, .. } => pane.foreground_query().map(|q| (i, q)),
            InfoSource::Stopped(_) => None,
        })
        .collect();
    let names = read_foreground_commands(&queries.iter().map(|&(_, q)| q).collect::<Vec<_>>());
    let mut fg: Vec<Option<String>> = vec![None; sources.len()];
    for ((i, _), name) in queries.iter().zip(names) {
        fg[*i] = name;
    }
    sources
        .into_iter()
        .zip(fg)
        .map(|(source, fg)| match source {
            InfoSource::Live {
                pane,
                workspace,
                window,
                labels,
            } => pane.info(&workspace, &window, &labels, fg),
            InfoSource::Stopped(info) => info,
        })
        .collect()
}

fn live_pane(registry: &PaneRegistry, pane_id: &str) -> RpcResult<Arc<Pane>> {
    registry.live(pane_id)
}

impl Core {
    pub(crate) fn pane_info(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneRefParams = params::parse("pane.info", raw)?;
        let snapshot = {
            let state = self.state();
            snapshot(&state, state.registry.get_or_throw(p.pane_id.as_str())?)?
        };
        Ok(render_infos(vec![snapshot]).remove(0))
    }

    pub(crate) fn pane_list(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneListParams = params::parse("pane.list", raw)?;
        let filter = p.filter.unwrap_or_default();
        let snapshots = {
            let state = self.state();
            let window_ids: Option<HashSet<String>> = filter
                .workspace
                .as_ref()
                .map(|w| state.layout.window_ids(w.as_str()).into_iter().collect());
            state
                .registry
                .filter(filter.state, filter.labels.as_ref(), window_ids.as_ref())
                .into_iter()
                .map(|entry| snapshot(&state, entry))
                .collect::<RpcResult<Vec<_>>>()?
        };
        Ok(Value::Array(render_infos(snapshots)))
    }

    pub(crate) fn pane_write(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneWriteParams = params::parse("pane.write", raw)?;
        p.require_one_payload("pane.write")?;
        let state = self.state(); // 入力・サイズの変更とイベントの発行を同じ lock の中で行う
        let pane = live_pane(&state.registry, p.pane_id.as_str())?;
        if pane.state() == RunState::Exited {
            return Err(pane_exited());
        }
        let data = match (p.data, p.data_b64) {
            (Some(text), _) => text.into_bytes(),
            (None, Some(b64)) => BASE64.decode(b64).map_err(|e| {
                RpcFailure::invalid_params(format!("invalid params for pane.write: dataB64: {e}"))
            })?,
            (None, None) => Vec::new(),
        };
        // 待ち行列が一杯なら、サイズの変更や入力のイベントより先に断る。
        let bytes = data.len();
        pane.check_input_room(bytes).map_err(input_queue_full)?;
        // 最後に操作したクライアントのサイズを優先する
        if let Some(client) = &p.client_id
            && pane.claim_size(client.as_str()).map_err(RpcFailure::internal)?
        {
            let (cols, rows) = pane.size();
            self.persist_soon();
            self.events.emit(
                "pane.resized",
                json!({"cols": cols, "rows": rows, "clientId": client.as_str()}),
                Some(&pane.id),
            );
        }
        pane.write(data).map_err(input_queue_full)?;
        // 内容は記録しない
        let source = p.source.unwrap_or(InputSource::Hub);
        self.events.emit(
            "input",
            json!({"source": source.as_str(), "bytes": bytes}),
            Some(&pane.id),
        );
        Ok(ok())
    }

    pub(crate) fn pane_resize(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneResizeParams = params::parse("pane.resize", raw)?;
        let state = self.state();
        let pane = live_pane(&state.registry, p.pane_id.as_str())?;
        let client = p.client_id.as_ref().map(params::Name::as_str);
        pane.resize(p.cols.get(), p.rows.get(), client)
            .map_err(RpcFailure::internal)?;
        self.persist_soon();
        self.events.emit(
            "pane.resized",
            json!({"cols": p.cols.get(), "rows": p.rows.get(), "clientId": client}),
            Some(&pane.id),
        );
        Ok(ok())
    }

    pub(crate) fn pane_screen(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneRefParams = params::parse("pane.screen", raw)?;
        let pane = live_pane(&self.state().registry, p.pane_id.as_str())?;
        let screen = pane.screen();
        Ok(json!({
            "text": screen.text,
            "cursor": {"x": screen.cursor.x, "y": screen.cursor.y},
            "altScreen": screen.alt_screen,
            "title": screen.title,
            "activity": screen.activity,
        }))
    }

    pub(crate) fn pane_subscribe_lines(&self, raw: Raw<'_>, ctx: &mut RequestContext) -> RpcResult<Value> {
        let p: SubscribeLinesParams = params::parse("pane.subscribe_lines", raw)?;
        let pane = live_pane(&self.state().registry, p.pane_id.as_str())?;
        // 接続ごとの購読キー。同じキーの再購読は既存を置き換える。
        let key = format!("lines:{}", pane.id);
        Ok(stream::subscribe(
            ctx,
            key,
            Arc::new(LinesSource(pane)),
            p.since.map(params::Seq::get),
            p.epoch.as_ref().map(params::Epoch::as_str),
            &self.epoch,
        ))
    }
}
