//! `Pane` の attach（出力の届け先の登録、再生、クライアントの管理とサイズの継承）。

use super::*;
use crate::params::Replay;

/// attach の結果。`attachment` は応答の後に `go_live` する。
pub struct Attached {
    pub attachment: Arc<Attachment>,
    pub head: u64,
    pub oldest: u64,
    pub truncated: bool,
}

impl Pane {
    /// 出力の届け先に加え、`clients` に載せる。届け先の登録と head の読み取りは、出力の取り込みと同じ lock（`order`）の中で
    /// 行うので、再生が覆う範囲（〜head）とライブで届く範囲（head〜）に隙間も重なりも出ない。
    pub fn attach(&self, conn: &Arc<ConnHandle>, client_id: &str, replay: Replay) -> Attached {
        let order = lock(&self.order);
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
        Attached {
            attachment,
            head,
            oldest,
            truncated,
        }
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

    pub fn remove_attachment(&self, attachment_id: u64) {
        lock(&self.attachments).retain(|a| a.id != attachment_id);
    }

    /// 全接続の attach を外して返す（close のとき、閉じた Pane への参照を残さない）。
    pub fn take_attachments(&self) -> Vec<Arc<Attachment>> {
        std::mem::take(&mut *lock(&self.attachments))
    }

    /// 同じ clientId が別の接続にも attach しているか。
    pub fn has_attachment_for(&self, client_id: &str) -> bool {
        lock(&self.attachments).iter().any(|a| a.client_id == client_id)
    }

    pub fn remove_client(&self, client_id: &str) {
        lock(&self.inner).clients.retain(|c| c != client_id);
    }

    /// クライアントの離脱。所有者だった場合は、残るクライアントのうち最後に操作したものの
    /// サイズを適用し、適用した先の clientId を返す（なければ None）。
    pub fn forget_client(&self, client_id: &str) -> Result<Option<String>, String> {
        let _order = lock(&self.order);
        let mut inner = lock(&self.inner);
        let Some(decision) = inner.sizes.forget(client_id) else {
            return Ok(None);
        };
        self.apply(&mut inner, &decision)?;
        Ok(decision.owner)
    }
}
