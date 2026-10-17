//! `Pane` の attach（出力の届け先の登録、再生、クライアントの管理とサイズの継承）。

use super::*;
use crate::params::Replay;

/// attach の結果。`attachment` は応答の後に `go_live` する。
pub struct Attached {
    pub attachment: Arc<Attachment>,
    pub head: u64,
    pub oldest: u64,
    pub truncated: bool,
    /// 同じ接続・同じ clientId の attach を置き換えた（クライアントとしては残っている）。
    pub replaced: bool,
    /// 同じ接続の別の clientId の attach を外した（その clientId と、サイズを継いだ先）。
    pub displaced: Option<Displaced>,
}

/// attach に置き換えられた、同じ接続の別のクライアント。
pub struct Displaced {
    pub client_id: String,
    pub inherited: Result<Option<String>, String>,
}

/// close が始まった pane への attach。
#[derive(Debug)]
pub struct AttachRefused;

impl Pane {
    /// 出力の届け先に加え、`clients` に載せる。届け先の登録と head の読み取りは、出力の取り込みと同じ lock（`order`）の中で
    /// 行うので、再生が覆う範囲（〜head）とライブで届く範囲（head〜）に隙間も重なりも出ない。
    /// close が始まっていれば Err（古い attach は pane 側にも接続側にもそのまま残る）。
    /// pane 側の登録・差し替え、接続側の map の登録・差し替え、close の解除は、すべて `order` の中で行う（1 つの線形化点）。
    pub fn attach(
        &self,
        conn: &Arc<ConnHandle>,
        client_id: &str,
        replay: Replay,
    ) -> Result<Attached, AttachRefused> {
        let order = lock(&self.order);
        if lock(&self.inner).attach_closed {
            return Err(AttachRefused);
        }
        let (head, oldest, truncated, entries) = {
            let inner = lock(&self.inner);
            let entries: Vec<(u64, String, Vec<u8>)> = match replay {
                Replay::Raw => inner
                    .raw
                    .since(0)
                    .map(|e| (e.seq, e.ts.clone(), e.item.clone()))
                    .collect(),
                Replay::Snapshot | Replay::None => Vec::new(),
            };
            (
                inner.raw.head(),
                inner.raw.oldest(),
                replay == Replay::Raw && inner.raw.has_gap(0),
                entries,
            )
        };
        let snapshot = (replay == Replay::Snapshot).then(|| self.observer.serialize_snapshot());
        let attachment = Attachment::new(&self.id, client_id, conn, head);
        lock(&self.attachments).push(Arc::clone(&attachment));
        #[cfg(all(test, unix))]
        self.attach_pause();
        // 接続側の map に登録し、同じ接続の古い attach を外す。
        let previous = conn.replace_attachment(&self.id, Arc::clone(&attachment));
        let replaced = previous.as_ref().is_some_and(|old| old.client_id == client_id);
        let displaced = previous.and_then(|old| {
            lock(&self.attachments).retain(|a| a.id != old.id);
            (!replaced).then(|| Displaced {
                inherited: self.forget_client_locked(&old.client_id),
                client_id: old.client_id.clone(),
            })
        });
        {
            let mut inner = lock(&self.inner);
            if !inner.clients.iter().any(|c| c == client_id) {
                inner.clients.push(client_id.to_string());
            }
        }
        drop(order);
        // 再生の行は lock の外で作る（応答の後まで、届いた出力は溜まる）。
        let mut lines: Vec<Vec<u8>> = entries
            .iter()
            .filter(|(seq, _, _)| *seq <= head)
            .map(|(seq, ts, data)| output_line(&self.id, *seq, ts, data, None))
            .collect();
        if let Some(text) = snapshot {
            lines.push(output_line(
                &self.id,
                head,
                &now_iso(),
                text.as_bytes(),
                Some("snapshot"),
            ));
        }
        attachment.set_replay(lines);
        Ok(Attached {
            attachment,
            head,
            oldest,
            truncated,
            replaced,
            displaced,
        })
    }

    /// 全 attach へ出力を配る（`order` の lock の中で呼ぶ）。
    pub(super) fn broadcast(&self, seq: u64, ts: &str, data: &[u8]) {
        let attachments = lock(&self.attachments);
        if attachments.is_empty() {
            return;
        }
        let line = output_line(&self.id, seq, ts, data, None);
        for attachment in attachments.iter() {
            attachment.deliver(seq, ts, data, &line);
        }
    }

    /// 同じ clientId が別の接続にも attach しているか。
    pub(crate) fn has_attachment_for(&self, client_id: &str) -> bool {
        lock(&self.attachments).iter().any(|a| a.client_id == client_id)
    }

    /// attach を外す。同じ clientId が別の接続に残っていなければ clients からも外し、サイズの所有者だったら
    /// 残ったクライアントのうち最後に操作したもののサイズへ戻す（その clientId を返す）。
    /// 外す・clients から消す・サイズを戻す、は `order` の下で 1 つの操作として行う（attach / close と交錯しない）。
    pub fn detach(&self, attachment_id: u64, client_id: &str) -> Result<Option<String>, String> {
        let _order = lock(&self.order);
        lock(&self.attachments).retain(|a| a.id != attachment_id);
        self.forget_client_locked(client_id)
    }

    /// クライアントの最後の attach が外れたとき（`order` を持っている間に呼ぶ）、clients とサイズの調停から外す。
    fn forget_client_locked(&self, client_id: &str) -> Result<Option<String>, String> {
        if self.has_attachment_for(client_id) {
            return Ok(None);
        }
        let mut inner = lock(&self.inner);
        inner.clients.retain(|c| c != client_id);
        let Some(decision) = inner.sizes.forget(client_id) else {
            return Ok(None);
        };
        self.apply(&mut inner, &decision)?;
        Ok(decision.owner)
    }

    /// close の開始。以後の attach は断る。
    pub(crate) fn refuse_attach(&self) {
        let _order = lock(&self.order);
        lock(&self.inner).attach_closed = true;
    }

    /// 全接続の attach を外して返す（close のとき、閉じた Pane への参照を残さない）。以後の attach は断る。
    /// 接続側の map からも、同じ `order` の中で外す。
    pub fn close_attachments(&self) -> Vec<Arc<Attachment>> {
        let _order = lock(&self.order);
        lock(&self.inner).clients.clear();
        let taken = std::mem::take(&mut *lock(&self.attachments));
        lock(&self.inner).attach_closed = true;
        for attachment in &taken {
            if let Some(conn) = attachment.conn() {
                conn.remove_attachment_if(&self.id, attachment.id);
            }
        }
        taken
    }
}

#[cfg(all(test, unix))]
impl Pane {
    /// 次の attach を、pane 側に登録した後・接続側に登録する前で止める。`arrived` で止まったことを知らせ、`release` で再開する。
    pub(crate) fn pause_next_attach(
        &self,
        arrived: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    ) {
        *lock(&self.attach_gate) = Some((arrived, release));
    }

    fn attach_pause(&self) {
        let gate = lock(&self.attach_gate).take();
        if let Some((arrived, release)) = gate {
            arrived.wait();
            release.wait();
        }
    }
}
