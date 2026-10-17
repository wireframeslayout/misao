//! ストリームごとの lastSeq と、デーモンの epoch。接続は持たない。

use std::collections::HashMap;

/// 購読できるストリーム。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StreamId {
    Events,
    Lines(String),
}

impl StreamId {
    pub fn key(&self) -> String {
        match self {
            Self::Events => "events".to_string(),
            Self::Lines(pane_id) => format!("lines:{pane_id}"),
        }
    }

    pub fn pane_id(&self) -> Option<&str> {
        match self {
            Self::Events => None,
            Self::Lines(id) => Some(id),
        }
    }
}

/// 利用側が指定する購読の開始位置。epoch は since と組で渡す（世代を比べるため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamPos {
    pub seq: u64,
    pub epoch: String,
}

#[derive(Debug, Default)]
pub struct StreamCursor {
    entries: HashMap<StreamId, u64>,
    epoch: Option<String>,
}

impl StreamCursor {
    pub fn contains(&self, stream: &StreamId) -> bool {
        self.entries.contains_key(stream)
    }

    pub fn streams(&self) -> Vec<StreamId> {
        self.entries.keys().cloned().collect()
    }

    pub fn last_seq(&self, stream: &StreamId) -> Option<u64> {
        self.entries.get(stream).copied()
    }

    pub fn add(&mut self, stream: StreamId, last_seq: u64) {
        self.entries.insert(stream, last_seq);
    }

    pub fn remove(&mut self, stream: &StreamId) {
        self.entries.remove(stream);
    }

    /// epoch を記録する。前回と違えば全ストリームの lastSeq を 0 に戻し、戻したものを返す。初回は何も戻さない。
    pub fn reset_for_epoch(&mut self, epoch: &str) -> Vec<StreamId> {
        let previous = self.epoch.replace(epoch.to_string());
        if previous.is_none() || previous.as_deref() == Some(epoch) {
            return Vec::new();
        }
        for seq in self.entries.values_mut() {
            *seq = 0;
        }
        self.streams()
    }

    /// 受信した seq を取り込む。lastSeq 以下（重複・登録なし）なら false を返し、配信しない。
    pub fn accept(&mut self, stream: &StreamId, seq: u64) -> bool {
        match self.entries.get_mut(stream) {
            Some(last) if seq > *last => {
                *last = seq;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_and_unregistered_streams_are_dropped() {
        let mut c = StreamCursor::default();
        let s = StreamId::Lines("p".into());
        assert!(!c.accept(&s, 1));
        c.add(s.clone(), 3);
        assert!(!c.accept(&s, 3));
        assert!(c.accept(&s, 4));
        assert_eq!(c.last_seq(&s), Some(4));
    }

    #[test]
    fn an_epoch_change_rewinds_every_stream_but_the_first_epoch_does_not() {
        let mut c = StreamCursor::default();
        assert!(c.reset_for_epoch("A").is_empty());
        c.add(StreamId::Events, 9);
        assert!(c.reset_for_epoch("A").is_empty());
        assert_eq!(c.reset_for_epoch("B"), vec![StreamId::Events]);
        assert_eq!(c.last_seq(&StreamId::Events), Some(0));
    }
}
