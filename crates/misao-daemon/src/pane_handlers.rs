//! pane.* の RPC（Node 版の `pane-lifecycle` と `pane-io` のうち、画面・attach を除いたもの）。

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use misao_protocol::ErrorCode;
use serde_json::{Map, Value, json};
use tokio::task::JoinSet;

use crate::core::{Core, State, to_value};
use crate::foreground::{ForegroundQuery, read_foreground_commands};
use crate::layout::ResolvedWindow;
use crate::pane::{LinesSource, Pane, PaneOpenOptions, RunState};
use crate::params::{
    self, InputSource, PaneListParams, PaneOpenParams, PaneRefParams, PaneResizeParams, PaneSetLabelParams,
    PaneWriteParams, SubscribeLinesParams,
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
    pub(crate) async fn pane_open(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneOpenParams = params::parse("pane.open", raw)?;
        if p.preplace.is_some() {
            return Err(RpcFailure::new(
                ErrorCode::NOT_IMPLEMENTED,
                "preplace is not implemented",
            ));
        }
        let labels = p.labels.unwrap_or_default();
        // 保存するのは env のみ。ephemeralEnv は spawn で使い切り、ここには渡らない。
        let env = p.env.clone().unwrap_or_default();
        let _gate = self.gate.lock().await;
        // layout（既定の遅延作成を含む）→ spawn → 登録 → 保存を 1 つの単位として扱う。失敗したら全て戻し、イベントは出さない。
        let (pane, target, before) = {
            let mut state = self.state();
            let before = state.layout.snapshot();
            let attempt = state
                .layout
                .resolve_window(p.window_id.as_ref().map(params::WindowId::as_str))
                .and_then(|target| {
                    let pane = Pane::spawn(PaneOpenOptions {
                        cmd: p.cmd,
                        cwd: p.cwd,
                        env: p.env.unwrap_or_default(),
                        ephemeral_env: p.ephemeral_env.unwrap_or_default(),
                        cols: p.cols.map(params::Dimension::get),
                        rows: p.rows.map(params::Dimension::get),
                        socket_path: &self.socket_path,
                        raw_ring_bytes: self.limits.raw_ring_bytes,
                        lines_ring_bytes: self.limits.lines_ring_bytes,
                        scrollback: self.limits.scrollback,
                        profiles: self.profiles.clone(),
                        events: Arc::clone(&self.events),
                    })
                    .map_err(|e| RpcFailure::invalid_params(format!("spawn failed: {e}")))?;
                    Ok((pane, target))
                });
            match attempt {
                Ok((pane, target)) => {
                    state.registry.add(PaneEntry {
                        pane_id: pane.id.clone(),
                        window_id: target.location.window.id.clone(),
                        labels: labels.clone(),
                        env,
                        body: Body::Live(Arc::clone(&pane)),
                    });
                    (pane, target, before)
                }
                Err(e) => {
                    state.layout.restore(before);
                    return Err(e);
                }
            }
        };
        if let Err(e) = self.persist() {
            {
                let mut state = self.state();
                state.layout.restore(before);
                state.registry.remove(&pane.id);
            }
            pane.abort_open().await;
            return Err(e);
        }
        self.announce_opened(&pane, &labels, &target);
        pane.start();
        self.sampler.register(&pane);
        Ok(json!({"paneId": pane.id}))
    }

    /// 既定の workspace / window の作成と `pane.opened` を出す。
    fn announce_opened(&self, pane: &Pane, labels: &BTreeMap<String, String>, target: &ResolvedWindow) {
        let workspace = &target.location.workspace;
        let window = &target.location.window;
        if target.created_workspace {
            self.events
                .emit("workspace.created", json!({"name": workspace}), None);
        }
        if target.created_window {
            self.events.emit(
                "window.created",
                json!({"windowId": window.id, "workspace": workspace, "name": window.name}),
                None,
            );
        }
        self.events.emit(
            "pane.opened",
            json!({
                "pid": pane.pid,
                "cmd": pane.cmd,
                "labels": labels,
                "workspace": workspace,
                "windowId": window.id,
            }),
            Some(&pane.id),
        );
    }

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

    pub(crate) async fn pane_set_label(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneSetLabelParams = params::parse("pane.set_label", raw)?;
        p.require_change("pane.set_label")?;
        let pane_id = p.pane_id.as_str();
        let _gate = self.gate.lock().await;
        let (before, labels) = {
            let mut state = self.state();
            let before = state.registry.get_or_throw(pane_id)?.labels.clone();
            let labels = state
                .registry
                .set_labels(pane_id, p.set.as_ref(), p.unset.as_deref())?;
            (before, labels)
        };
        // 保存に失敗したらラベルを元に戻す。
        if let Err(e) = self.persist() {
            self.state().registry.replace_labels(pane_id, before);
            return Err(e);
        }
        self.events.emit(
            "pane.label",
            json!({"set": p.set.unwrap_or_default(), "unset": p.unset.unwrap_or_default()}),
            Some(pane_id),
        );
        Ok(json!({"labels": labels}))
    }

    pub(crate) async fn pane_close(&self, raw: Raw<'_>) -> RpcResult<Value> {
        let p: PaneRefParams = params::parse("pane.close", raw)?;
        self.close_pane(p.pane_id.as_str()).await?;
        Ok(ok())
    }

    /// pane.close と同じ手順で閉じる（終了を待ち、登録を外し、保存して `pane.closed` を出す）。
    /// live はプロセスを戻せないので、保存に失敗してもメモリは実態（閉じ済み）のままにして、`pane.closed` を出してから失敗を返す。
    /// stopped は記録を消すだけで、保存に失敗したら記録を戻す。
    pub(crate) async fn close_pane(&self, pane_id: &str) -> RpcResult<()> {
        let live = {
            let state = self.state();
            match &state.registry.get_or_throw(pane_id)?.body {
                Body::Live(pane) => Some(Arc::clone(pane)),
                Body::Stopped(_) => None,
            }
        };
        let Some(pane) = live else {
            return self.close_stopped(pane_id).await;
        };
        pane.close().await;
        let _gate = self.gate.lock().await;
        {
            // 並行した close が後始末済み
            let state = self.state();
            if !state
                .registry
                .get(pane_id)
                .and_then(PaneEntry::live)
                .is_some_and(|live| Arc::ptr_eq(live, &pane))
            {
                return Ok(());
            }
        }
        self.release_attachments(&pane); // client.detached は pane.closed より前に出す
        self.state().registry.remove(pane_id);
        pane.release();
        let saved = self.persist();
        self.events.emit("pane.closed", json!({}), Some(pane_id));
        saved
    }

    async fn close_stopped(&self, pane_id: &str) -> RpcResult<()> {
        let _gate = self.gate.lock().await;
        let removed = {
            let mut state = self.state();
            let index = state.registry.index_of(pane_id);
            index.and_then(|i| state.registry.remove(pane_id).map(|entry| (i, entry)))
        };
        let Some((index, entry)) = removed else {
            return Ok(()); // 並行した close が後始末済み
        };
        if let Err(e) = self.persist() {
            self.state().registry.insert_at(index, entry);
            return Err(e);
        }
        self.events.emit("pane.closed", json!({}), Some(pane_id));
        Ok(())
    }

    /// 配下の pane を全部閉じる。閉じている間に開かれた pane も残さない。
    pub(crate) async fn close_panes_in(self: &Arc<Self>, window_ids: &HashSet<String>) -> RpcResult<()> {
        loop {
            let ids: Vec<String> = self
                .state()
                .registry
                .filter(None, None, Some(window_ids))
                .into_iter()
                .map(|e| e.pane_id.clone())
                .collect();
            if ids.is_empty() {
                return Ok(());
            }
            let mut closing = JoinSet::new();
            for id in ids {
                let core = Arc::clone(self);
                closing.spawn(async move { core.close_pane(&id).await });
            }
            while let Some(result) = closing.join_next().await {
                result.map_err(|e| RpcFailure::internal(e.to_string()))??;
            }
        }
    }

    /// デーモンの終了。生きている pane を全部閉じる（`pane.closed` は出さない: 再起動後も pane の記録は残る）。
    pub async fn shutdown(&self) {
        // 記録は消さない（再起動後に stopped として復元される）。遅らせていたサイズの保存だけを済ませる。
        {
            let gate = self.gate.lock().await;
            self.flush_persist(&gate);
        }
        let panes = self.state().registry.panes();
        let mut closing = JoinSet::new();
        for pane in panes {
            closing.spawn(async move {
                pane.close().await;
                pane.release();
            });
        }
        while closing.join_next().await.is_some() {}
    }
}
