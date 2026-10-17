use std::collections::VecDeque;

#[derive(Debug)]
pub struct RingEntry<T> {
    pub seq: u64,
    pub ts: String,
    size: usize,
    pub item: T,
}

/// seq 付きリングバッファ（Node 版の `SeqRing`）。容量は size 単位（バイト数や件数）で制限する。
/// 1 件が容量を超えていても直近 1 件は必ず残す。seq は 1 から連続して増える。
#[derive(Debug)]
pub struct SeqRing<T> {
    entries: VecDeque<RingEntry<T>>,
    capacity: usize,
    total: usize,
    head: u64,
}

impl<T> SeqRing<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            total: 0,
            head: 0,
        }
    }

    pub fn head(&self) -> u64 {
        self.head
    }

    /// 保持している最古の seq。空なら head + 1。
    pub fn oldest(&self) -> u64 {
        self.entries.front().map_or(self.head + 1, |e| e.seq)
    }

    pub fn push(&mut self, item: T, size: usize, ts: String) -> u64 {
        self.head += 1;
        let seq = self.head;
        self.entries.push_back(RingEntry { seq, ts, size, item });
        self.total += size;
        while self.total > self.capacity && self.entries.len() > 1 {
            if let Some(evicted) = self.entries.pop_front() {
                self.total -= evicted.size;
            }
        }
        seq
    }

    /// seq のエントリ。保持範囲外（追い出し済み・head より先）なら None。O(1)。
    pub fn entry(&self, seq: u64) -> Option<&RingEntry<T>> {
        let oldest = self.oldest();
        if seq < oldest || seq > self.head {
            return None;
        }
        self.entries.get(usize::try_from(seq - oldest).ok()?)
    }

    /// since が保持範囲より古い、または head より先（= 送信側が再起動して seq が巻き戻った）なら欠落扱い。
    pub fn has_gap(&self, since: u64) -> bool {
        since + 1 < self.oldest() || since > self.head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(capacity: usize) -> SeqRing<&'static str> {
        SeqRing::new(capacity)
    }

    #[test]
    fn empty_ring_has_oldest_one_past_head() {
        let r = ring(10);
        assert_eq!((r.head(), r.oldest()), (0, 1));
        assert!(!r.has_gap(0));
        assert!(r.has_gap(1), "since ahead of head is a gap");
    }

    #[test]
    fn seq_is_contiguous_and_entries_are_found_by_seq() {
        let mut r = ring(100);
        for (i, item) in ["a", "b", "c"].into_iter().enumerate() {
            assert_eq!(r.push(item, 1, format!("t{i}")), i as u64 + 1);
        }
        assert_eq!(
            r.entry(2).map(|e| (e.seq, e.item, e.ts.as_str())),
            Some((2, "b", "t1"))
        );
        assert!(r.entry(0).is_none() && r.entry(4).is_none());
    }

    #[test]
    fn evicts_oldest_by_size_and_reports_gap() {
        let mut r = ring(3);
        for item in ["a", "b", "c", "d", "e"] {
            r.push(item, 1, String::new());
        }
        assert_eq!((r.oldest(), r.head()), (3, 5));
        assert!(r.entry(2).is_none());
        assert_eq!(r.entry(3).map(|e| e.item), Some("c"));
        assert!(!r.has_gap(2), "since = oldest - 1 is still contiguous");
        assert!(r.has_gap(1));
        assert!(!r.has_gap(5));
        assert!(r.has_gap(6));
    }

    #[test]
    fn keeps_the_latest_entry_even_when_it_exceeds_capacity() {
        let mut r = ring(2);
        r.push("small", 1, String::new());
        r.push("huge", 100, String::new());
        assert_eq!((r.oldest(), r.head()), (2, 2));
        assert_eq!(r.entry(2).map(|e| e.item), Some("huge"));
    }
}
