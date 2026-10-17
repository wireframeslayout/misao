use std::collections::VecDeque;

#[derive(Debug)]
pub struct RingEntry<T> {
    pub seq: u64,
    pub ts: String,
    size: usize,
    /// このエントリまでに push した size の累計（追い出されたぶんも含む）。
    cumulative: u64,
    pub item: T,
}

/// seq 付きリングバッファ（Node 版の `SeqRing`）。容量は size 単位（バイト数や件数）で制限する。
/// 1 件が容量を超えていても直近 1 件は必ず残す。seq は 1 から連続して増える。
#[derive(Debug)]
pub struct SeqRing<T> {
    entries: VecDeque<RingEntry<T>>,
    capacity: usize,
    total: usize,
    /// これまでに push した size の累計。
    pushed: u64,
    head: u64,
}

impl<T> SeqRing<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            total: 0,
            pushed: 0,
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
        self.pushed += size as u64;
        self.entries.push_back(RingEntry {
            seq,
            ts,
            size,
            cumulative: self.pushed,
            item,
        });
        self.total += size;
        while self.total > self.capacity && self.entries.len() > 1 {
            if let Some(evicted) = self.entries.pop_front() {
                self.total -= evicted.size;
            }
        }
        seq
    }

    /// seq までに push した size の累計。0 は常に 0。保持範囲外（追い出し済み・head より先）なら None。
    /// 2 つの seq の差が、その間に積まれたぶんの size になる。
    pub fn cumulative_at(&self, seq: u64) -> Option<u64> {
        if seq == 0 {
            return Some(0);
        }
        self.entry(seq).map(|e| e.cumulative)
    }

    /// head までに push した size の累計。
    pub fn cumulative_head(&self) -> u64 {
        self.pushed
    }

    /// seq > since のエントリ（古い順）。
    pub fn since(&self, since: u64) -> impl Iterator<Item = &RingEntry<T>> {
        let skip = since.saturating_add(1).saturating_sub(self.oldest());
        self.entries
            .iter()
            .skip(usize::try_from(skip).unwrap_or(usize::MAX))
    }

    /// since が保持範囲より古い、または head より先（= 送信側が再起動して seq が巻き戻った）なら欠落扱い。
    pub fn has_gap(&self, since: u64) -> bool {
        since + 1 < self.oldest() || since > self.head
    }

    /// seq のエントリ。保持範囲外（追い出し済み・head より先）なら None。O(1)。
    pub fn entry(&self, seq: u64) -> Option<&RingEntry<T>> {
        let oldest = self.oldest();
        if seq < oldest || seq > self.head {
            return None;
        }
        self.entries.get(usize::try_from(seq - oldest).ok()?)
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
    fn evicts_oldest_by_size() {
        let mut r = ring(3);
        for item in ["a", "b", "c", "d", "e"] {
            r.push(item, 1, String::new());
        }
        assert_eq!((r.oldest(), r.head()), (3, 5));
        assert!(r.entry(2).is_none());
        assert_eq!(r.entry(3).map(|e| e.item), Some("c"));
    }

    #[test]
    fn cumulative_sizes_measure_the_distance_between_two_seqs_even_after_eviction() {
        let mut r = ring(3);
        for item in ["a", "b", "c", "d", "e"] {
            r.push(item, 1, String::new());
        }
        assert_eq!(r.cumulative_head(), 5);
        assert_eq!(r.cumulative_at(5), Some(5));
        assert_eq!(r.cumulative_at(3), Some(3));
        assert_eq!(r.cumulative_at(2), None, "evicted");
        assert_eq!(r.cumulative_at(0), Some(0));
        assert_eq!(r.cumulative_at(9), None, "ahead of head");
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
