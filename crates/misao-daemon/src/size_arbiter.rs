/// 適用すべきサイズの決定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeDecision {
    pub cols: u32,
    pub rows: u32,
    /// このサイズを決めたクライアント。clientId なしの resize では None。
    pub owner: Option<String>,
}

/// 「最後に操作したクライアントのサイズを優先する」調停。pty には依存せず、
/// 適用すべきサイズを返すだけ（適用は呼び出し側）。Node 版の `SizeArbiter`。
#[derive(Debug, Default)]
pub struct SizeArbiter {
    /// 並び順 = 操作の新しさ（末尾が最新）。
    sizes: Vec<(String, u32, u32)>,
    owner: Option<String>,
}

impl SizeArbiter {
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// client のサイズを記録し、そのクライアントを所有者にする。client なしは所有者を None にする。
    pub fn record(&mut self, client: Option<&str>, cols: u32, rows: u32) -> SizeDecision {
        if let Some(id) = client {
            self.sizes.retain(|(known, _, _)| known != id);
            self.sizes.push((id.to_string(), cols, rows));
        }
        self.owner = client.map(str::to_string);
        SizeDecision {
            cols,
            rows,
            owner: self.owner.clone(),
        }
    }

    /// client が所有者でなく、サイズを記録済みなら、そのサイズへ戻す。変更なしは None。
    pub fn claim(&mut self, client: &str) -> Option<SizeDecision> {
        let (_, cols, rows) = self.sizes.iter().find(|(known, _, _)| known == client)?;
        let (cols, rows) = (*cols, *rows);
        if self.owner.as_deref() == Some(client) {
            return None;
        }
        Some(self.record(Some(client), cols, rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_makes_the_client_the_owner_and_no_client_clears_it() {
        let mut a = SizeArbiter::default();
        let d = a.record(Some("A"), 120, 40);
        assert_eq!((d.cols, d.rows, d.owner.as_deref()), (120, 40, Some("A")));
        assert_eq!(a.owner(), Some("A"));
        let d = a.record(None, 80, 24);
        assert_eq!(d.owner, None);
        assert_eq!(a.owner(), None);
    }

    #[test]
    fn claim_returns_the_recorded_size_unless_already_the_owner() {
        let mut a = SizeArbiter::default();
        a.record(Some("A"), 120, 40);
        a.record(Some("B"), 80, 24);
        assert_eq!(a.owner(), Some("B"));
        assert_eq!(a.claim("B"), None, "owner already");
        assert_eq!(a.claim("unknown"), None, "no recorded size");
        let d = a.claim("A").unwrap();
        assert_eq!((d.cols, d.rows, d.owner.as_deref()), (120, 40, Some("A")));
        assert_eq!(a.owner(), Some("A"));
    }

    #[test]
    fn a_resize_without_client_keeps_recorded_sizes_claimable() {
        let mut a = SizeArbiter::default();
        a.record(Some("A"), 120, 40);
        a.record(None, 50, 10);
        assert_eq!(a.claim("A").map(|d| (d.cols, d.rows)), Some((120, 40)));
    }
}
