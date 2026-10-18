//! デーモンの中核。接続に依存しない状態（配置・イベント）と RPC の実装を持つ。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use std::time::Instant;
use tokio::sync::Mutex as AsyncMutex;

use misao_protocol::{ErrorCode, PROTOCOL_SCHEMA, PROTOCOL_VERSION, ServerInfo};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::event_log::{EventLog, EventsSource};
use crate::foreground_sampler::ForegroundSampler;
use crate::layout::Layout;
use crate::limits::Limits;
use crate::log::Logger;
use crate::params::{self, Empty, EventsSubscribeParams};
use crate::persistence::{PersistedState, Version1, load_persisted_state};
use crate::profile::Profile;
use crate::registry::PaneRegistry;
use crate::rpc::RequestContext;
use crate::rpc_error::{RpcFailure, RpcResult};
use crate::state_persister::StatePersister;
use crate::stream;

pub struct CoreOptions {
    /// デーモンのビルド版（リリースの version）。指定すると `server.info` の version で報告する。
    pub version: Option<String>,
    /// 子プロセスの環境変数 `MISAO_SOCKET` に渡す、接続口のパス。
    pub socket_path: PathBuf,
    pub limits: Limits,
    pub log: Logger,
    /// 状態の保存先（`persistence.json`）。起動時にここから復元する。None なら保存も復元もしない
    /// （ソケットを持たない組み込み・テスト用）。
    pub state_path: Option<PathBuf>,
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
    /// 状態の保存。`state_path` が無ければ None。
    pub(crate) persister: Option<Arc<StatePersister>>,
    /// 保存する状態を変える操作（open / close / label / layout）を 1 つずつ行うための排他。
    /// 状態を変えてから保存し、失敗したら戻すまでの間に、他の変更が割り込まないようにする。
    /// fsync は全体の状態の lock の外で行うので、読み取り・入出力はこれを待たない。
    /// lock の順序: gate -> 保存の書き込み -> 全体の状態。
    pub(crate) gate: AsyncMutex<()>,
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
    /// 起動世代（`server.info` の epoch）。
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// 状態を復元して中核を作る。保存先のファイルが壊れていれば Err（起動を止める）。
    pub fn new(opts: CoreOptions) -> Result<Arc<Self>, String> {
        let CoreOptions {
            version,
            socket_path,
            limits,
            log,
            state_path,
            profiles,
        } = opts;
        let restored = state_path
            .as_deref()
            .map(load_persisted_state)
            .transpose()?
            .unwrap_or_else(PersistedState::empty);
        let mut layout = Layout::default();
        layout.restore_persisted(restored.workspaces);
        let mut registry = PaneRegistry::default();
        registry.restore_stopped(restored.panes);
        let persister = state_path.map(|path| Arc::new(StatePersister::new(&path, log.clone())));
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
            state: Mutex::new(State { layout, registry }),
            persister,
            gate: AsyncMutex::new(()),
        });
        core.start_persist_timer();
        core.events.emit(
            "daemon.started",
            json!({"pid": std::process::id(), "protocolVersion": PROTOCOL_VERSION}),
            None,
        );
        Ok(core)
    }

    /// 遅らせた保存（サイズ）の期限が来たら保存するスレッド。Core が捨てられると止まる。
    fn start_persist_timer(self: &Arc<Self>) {
        let Some(persister) = self.persister.clone() else {
            return;
        };
        let core = Arc::downgrade(self);
        std::thread::spawn(move || {
            persister.run_timer(|| {
                if let Some(core) = core.upgrade() {
                    core.flush_persist(&core.gate.blocking_lock());
                }
            });
        });
    }

    /// 遅らせている保存があれば今すぐ行う。失敗はログだけ（RPC は落とさない）。
    pub(crate) fn flush_persist(&self, _gate: &tokio::sync::MutexGuard<'_, ()>) {
        let Some(persister) = &self.persister else {
            return;
        };
        if persister.has_pending()
            && let Err(e) = self.persist()
        {
            persister.log_failure(&e.message);
        }
    }

    /// 今の状態を保存する（`gate` を持っている間に呼ぶ）。失敗は呼び出し側が戻す。
    pub(crate) fn persist(&self) -> RpcResult<()> {
        let Some(persister) = &self.persister else {
            return Ok(());
        };
        // 書き込みの順序づけを取ってからスナップショットを作る（古い状態が新しい状態を上書きしない）。
        let guard = persister.write_guard();
        let generation = persister.generation();
        let snapshot = {
            let state = self.state();
            PersistedState {
                version: Version1,
                workspaces: state.layout.to_persisted().map_err(RpcFailure::internal)?,
                panes: state.registry.to_persisted().map_err(RpcFailure::internal)?,
            }
        };
        persister
            .save(&guard, generation, &snapshot)
            .map_err(RpcFailure::internal)
    }

    /// サイズのような、再起動後の表示用の付帯情報の保存を遅らせて予約する。
    pub(crate) fn persist_soon(&self) {
        if let Some(persister) = &self.persister {
            persister.save_soon();
        }
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
            "workspace.create" => self.workspace_create(raw).await,
            "workspace.rename" => self.workspace_rename(raw).await,
            "workspace.close" => self.workspace_close(raw).await,
            "window.create" => self.window_create(raw).await,
            "window.rename" => self.window_rename(raw).await,
            "window.close" => self.window_close(raw).await,
            "events.subscribe" => self.events_subscribe(raw, ctx),
            "pane.open" => self.pane_open(raw).await,
            "pane.info" => self.pane_info(raw),
            "pane.list" => self.pane_list(raw),
            "pane.write" => self.pane_write(raw),
            "pane.resize" => self.pane_resize(raw),
            "pane.screen" => self.pane_screen(raw),
            "pane.subscribe_lines" => self.pane_subscribe_lines(raw, ctx),
            "pane.set_label" => self.pane_set_label(raw).await,
            "pane.attach" => self.pane_attach(raw, ctx),
            "pane.detach" => self.pane_detach(raw, ctx),
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

impl Drop for Core {
    fn drop(&mut self) {
        if let Some(persister) = &self.persister {
            persister.stop();
        }
    }
}
