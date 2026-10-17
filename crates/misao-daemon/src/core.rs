//! デーモンの中核。接続に依存しない状態（配置・イベント）と RPC の実装を持つ。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use misao_protocol::{ErrorCode, PROTOCOL_SCHEMA, PROTOCOL_VERSION, ServerInfo};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::event_log::{EventLog, EventsSource};
use crate::foreground_sampler::ForegroundSampler;
use crate::layout::Layout;
use crate::limits::Limits;
use crate::log::Logger;
use crate::params::{self, Empty, EventsSubscribeParams};
use crate::profile::Profile;
use crate::registry::PaneRegistry;
use crate::rpc::RequestContext;
use crate::rpc_error::{RpcFailure, RpcResult};
use crate::stream;

pub struct CoreOptions {
    /// デーモンのビルド版（リリースの version）。指定すると `server.info` の version で報告する。
    pub version: Option<String>,
    /// 子プロセスの環境変数 `MISAO_SOCKET` に渡す、接続口のパス。
    pub socket_path: PathBuf,
    pub limits: Limits,
    pub log: Logger,
    /// pane の稼働判定に使うプロファイル（`pane.open` の cmd で最初に一致したものを使う）。
    pub profiles: Vec<Arc<Profile>>,
}

pub(crate) struct State {
    pub layout: Layout,
    pub registry: PaneRegistry,
}

pub struct Core {
    version: Option<String>,
    /// 起動ごとに変わる ID。seq の巻き戻り（デーモン再起動）をクライアントが検知するためのもの。
    pub(crate) epoch: String,
    started_at: Instant,
    pub(crate) log: Logger,
    pub(crate) events: Arc<EventLog>,
    state: Mutex<State>,
    /// 子プロセスの環境変数 `MISAO_SOCKET` に渡す、接続口のパス。
    pub(crate) socket_path: PathBuf,
    pub(crate) limits: Limits,
    pub(crate) profiles: Vec<Arc<Profile>>,
    /// 前面のプログラムの定期調査（profile の選び直し）。デーモンに 1 つ。
    pub(crate) sampler: ForegroundSampler,
}

pub(crate) fn to_value<T: Serialize>(value: &T) -> RpcResult<Value> {
    serde_json::to_value(value).map_err(|e| RpcFailure::internal(e.to_string()))
}

/// `server.schema` の結果。埋め込みのスキーマを最初の呼び出しで一度だけ読む。
fn protocol_schema() -> RpcResult<Value> {
    static SCHEMA: OnceLock<Result<Value, String>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| serde_json::from_str(PROTOCOL_SCHEMA).map_err(|e| e.to_string()))
        .clone()
        .map_err(RpcFailure::internal)
}

impl Core {
    pub fn new(opts: CoreOptions) -> Arc<Self> {
        let CoreOptions {
            version,
            socket_path,
            limits,
            log,
            profiles,
        } = opts;
        let core = Arc::new(Self {
            version,
            epoch: crate::ids::new_ulid(),
            started_at: Instant::now(),
            log,
            socket_path,
            limits,
            profiles,
            sampler: ForegroundSampler::start(),
            events: Arc::new(EventLog::new(limits.events)),
            state: Mutex::new(State {
                layout: Layout::default(),
                registry: PaneRegistry::default(),
            }),
        });
        core.events.emit(
            "daemon.started",
            json!({"pid": std::process::id(), "protocolVersion": PROTOCOL_VERSION}),
            None,
        );
        core
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) async fn call(
        self: &Arc<Self>,
        method: &str,
        raw: Option<&Map<String, Value>>,
        ctx: &mut RequestContext,
    ) -> RpcResult<Value> {
        match method {
            "server.info" => {
                params::parse::<Empty>(method, raw)?;
                to_value(&self.server_info())
            }
            "server.schema" => {
                params::parse::<Empty>(method, raw)?;
                protocol_schema()
            }
            "workspace.list" => self.workspace_list(raw),
            "workspace.create" => self.workspace_create(raw),
            "workspace.rename" => self.workspace_rename(raw),
            "workspace.close" => self.workspace_close(raw).await,
            "window.create" => self.window_create(raw),
            "window.rename" => self.window_rename(raw),
            "window.close" => self.window_close(raw).await,
            "events.subscribe" => self.events_subscribe(raw, ctx),
            "pane.open" => self.pane_open(raw),
            "pane.info" => self.pane_info(raw),
            "pane.list" => self.pane_list(raw),
            "pane.write" => self.pane_write(raw),
            "pane.resize" => self.pane_resize(raw),
            "pane.screen" => self.pane_screen(raw),
            "pane.subscribe_lines" => self.pane_subscribe_lines(raw, ctx),
            "pane.set_label" => self.pane_set_label(raw),
            "pane.close" => self.pane_close(raw).await,
            _ => Err(RpcFailure::new(
                ErrorCode::NOT_IMPLEMENTED,
                format!("not implemented: {method}"),
            )),
        }
    }

    /// 1 接続につき events のストリームは 1 つ。再度の購読は既存を置き換える。
    fn events_subscribe(
        &self,
        raw: Option<&Map<String, Value>>,
        ctx: &mut RequestContext,
    ) -> RpcResult<Value> {
        let p: EventsSubscribeParams = params::parse("events.subscribe", raw)?;
        Ok(stream::subscribe(
            ctx,
            "events".to_string(),
            Arc::new(EventsSource(Arc::clone(&self.events))),
            p.since.map(params::Seq::get),
            p.epoch.as_ref().map(params::Epoch::as_str),
            &self.epoch,
        ))
    }

    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: PROTOCOL_VERSION.to_string(),
            version: self.version.clone(),
            pid: std::process::id(),
            epoch: self.epoch.clone(),
            uptime_sec: self.started_at.elapsed().as_secs(),
            pane_count: self.state().registry.len() as u64,
            event_head: self.events.head(),
        }
    }
}
