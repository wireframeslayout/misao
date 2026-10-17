//! `pane.attach` / `pane.detach`（Node 版の `pane-io` の attach / detach）。
//! 複数のクライアントが 1 つの pane に入れる。pane のサイズは最後に操作したクライアントのものになり、
//! 所有者が抜けたら残ったクライアントのうち最後に操作したもののサイズに戻る。

use std::sync::Arc;

use misao_protocol::ErrorCode;
use serde_json::{Map, Value, json};

use crate::connection::ConnHandle;
use crate::core::Core;
use crate::pane::Pane;
use crate::params::{self, AttachMode, PaneAttachParams, PaneRefParams};
use crate::rpc::RequestContext;
use crate::rpc_error::{RpcFailure, RpcResult};

type Raw<'a> = Option<&'a Map<String, Value>>;

impl Core {
    /// 応答の後に再生とライブへの切り替えを行う（応答より先に `pane.output` を出さない）。
    pub(crate) fn pane_attach(&self, raw: Raw<'_>, ctx: &mut RequestContext) -> RpcResult<Value> {
        let p: PaneAttachParams = params::parse("pane.attach", raw)?;
        if p.mode == AttachMode::Cells {
            return Err(RpcFailure::new(
                ErrorCode::UNSUPPORTED,
                "mode \"cells\" is not supported",
            ));
        }
        let pane = self.state().registry.live(p.pane_id.as_str())?;
        let conn = Arc::clone(&ctx.conn);
        let client_id = p.client_id.as_str();
        let previous = conn.attachment(&pane.id);
        let replacing = previous.as_ref().is_some_and(|a| a.client_id == client_id);
        match previous {
            // 同じ接続・同じ clientId の再 attach: 出力の届け先だけを差し替える（クライアントとしては残し、client.detached も出さない）。
            Some(old) if replacing => {
                conn.take_attachment(&pane.id);
                pane.remove_attachment(old.id);
            }
            _ => self.detach_from(&conn, &pane.id),
        }
        if let (Some(cols), Some(rows)) = (p.cols, p.rows) {
            pane.resize(cols.get(), rows.get(), Some(client_id))
                .map_err(RpcFailure::internal)?;
            self.persist_soon();
            self.events.emit(
                "pane.resized",
                json!({"cols": cols.get(), "rows": rows.get(), "clientId": client_id}),
                Some(&pane.id),
            );
        }
        let attached = pane.attach(&conn, client_id, p.replay);
        conn.set_attachment(&pane.id, Arc::clone(&attached.attachment));
        if !replacing {
            self.events
                .emit("client.attached", json!({"clientId": client_id}), Some(&pane.id));
        }
        let attachment = Arc::clone(&attached.attachment);
        let pane_id = pane.id.clone();
        if conn.is_closed() {
            // 登録している間に接続が閉じた（`connection_closed` の走査の後に登録した attach を残さない）。
            self.detach_from(&conn, &pane_id);
        }
        ctx.after_reply(move || {
            // 応答前に同じ接続で再 attach / detach されたら、この attach の再生とキューは捨てる。
            if conn
                .attachment(&pane_id)
                .is_some_and(|current| current.id == attachment.id)
            {
                attachment.go_live();
            }
        });
        Ok(json!({"head": attached.head, "oldest": attached.oldest, "truncated": attached.truncated}))
    }

    pub(crate) fn pane_detach(&self, raw: Raw<'_>, ctx: &RequestContext) -> RpcResult<Value> {
        let p: PaneRefParams = params::parse("pane.detach", raw)?;
        self.detach_from(&ctx.conn, p.pane_id.as_str());
        Ok(json!({"ok": true}))
    }

    /// 接続の attach を外す。接続の切断からも呼ばれるので、サイズの継承の保存は遅らせる（失敗しても RPC は落とさない）。
    pub(crate) fn detach_from(&self, conn: &ConnHandle, pane_id: &str) {
        let Some(attachment) = conn.take_attachment(pane_id) else {
            return;
        };
        let pane = self.state().registry.live(pane_id).ok();
        if let Some(pane) = pane {
            pane.remove_attachment(attachment.id);
            // 同じ clientId が別の接続にも残っていれば clients から外さない
            if !pane.has_attachment_for(&attachment.client_id) {
                pane.remove_client(&attachment.client_id);
                self.inherit_size(&pane, &attachment.client_id);
            }
        }
        self.events.emit(
            "client.detached",
            json!({"clientId": attachment.client_id}),
            Some(pane_id),
        );
    }

    /// 抜けたクライアントがサイズの所有者だったとき、残ったクライアントのうち最後に操作したものにサイズを戻す。
    fn inherit_size(&self, pane: &Pane, client_id: &str) {
        match pane.forget_client(client_id) {
            Ok(Some(owner)) => {
                let (cols, rows) = pane.size();
                self.events.emit(
                    "pane.resized",
                    json!({"cols": cols, "rows": rows, "clientId": owner}),
                    Some(&pane.id),
                );
                self.persist_soon();
            }
            Ok(None) => {}
            Err(e) => self
                .log
                .error(&format!("cannot restore the pane size after detach: {e}")),
        }
    }

    /// 接続が閉じた。attach していた pane から外す。
    pub(crate) fn connection_closed(&self, conn: &ConnHandle) {
        for pane_id in conn.attached_pane_ids() {
            self.detach_from(conn, &pane_id);
        }
    }

    /// pane の close で、全接続の attach を外す（閉じた Pane への参照を残さない）。
    pub(crate) fn release_attachments(&self, pane: &Pane) {
        for attachment in pane.take_attachments() {
            if let Some(conn) = attachment.conn() {
                conn.remove_attachment_if(&pane.id, attachment.id);
            }
            pane.remove_client(&attachment.client_id);
            self.events.emit(
                "client.detached",
                json!({"clientId": attachment.client_id}),
                Some(&pane.id),
            );
        }
    }
}
