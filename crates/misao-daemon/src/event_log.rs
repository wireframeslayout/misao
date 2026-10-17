use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use tokio::sync::watch;

use crate::log::now_iso;
use crate::ring::SeqRing;
use crate::stream::{Next, StreamSource};

#[derive(Debug, Clone, PartialEq)]
pub struct DaemonEvent {
    pub kind: String,
    pub data: Value,
    /// pane に紐づくイベントのみ。data ではなく外側に載せる。
    pub pane_id: Option<String>,
}

/// デーモン全体のイベント: seq 付きリング + ライブ配信（Node 版の `EventLog`）。
/// 購読者は `wake()` の受信側で新着を待ち、リングから pull する。
pub struct EventLog {
    ring: Mutex<SeqRing<DaemonEvent>>,
    head_tx: watch::Sender<u64>,
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            ring: Mutex::new(SeqRing::new(capacity)),
            head_tx: watch::Sender::new(0),
        }
    }

    pub fn head(&self) -> u64 {
        self.with_ring(|ring| ring.head())
    }

    pub fn emit(&self, kind: &str, data: Value, pane_id: Option<&str>) {
        let event = DaemonEvent {
            kind: kind.to_string(),
            data,
            pane_id: pane_id.map(str::to_string),
        };
        let seq = self.with_ring(|ring| ring.push(event, 1, now_iso()));
        self.head_tx.send_replace(seq);
    }

    /// 新着の通知。値は head の seq。
    pub fn wake(&self) -> watch::Receiver<u64> {
        self.head_tx.subscribe()
    }

    pub fn with_ring<R>(&self, f: impl FnOnce(&mut SeqRing<DaemonEvent>) -> R) -> R {
        f(&mut self.ring.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// `events.subscribe` の購読元。
pub struct EventsSource(pub Arc<EventLog>);

impl StreamSource for EventsSource {
    fn bounds(&self) -> (u64, u64) {
        self.0.with_ring(|ring| (ring.head(), ring.oldest()))
    }

    fn next(&self, cursor: u64, writable: bool) -> Next {
        self.0.with_ring(|ring| {
            if cursor + 1 < ring.oldest() {
                return Next::Overtaken;
            }
            if cursor >= ring.head() {
                return Next::Empty;
            }
            if !writable {
                return Next::Blocked;
            }
            let Some(entry) = ring.entry(cursor + 1) else {
                return Next::Overtaken;
            };
            let event = &entry.item;
            let mut params = json!({
                "seq": entry.seq,
                "ts": entry.ts,
                "type": event.kind,
                "data": event.data,
            });
            if let (Some(pane_id), Some(map)) = (&event.pane_id, params.as_object_mut()) {
                map.insert("paneId".to_string(), json!(pane_id));
            }
            Next::Entry {
                seq: entry.seq,
                notification: json!({"jsonrpc": "2.0", "method": "event", "params": params}),
            }
        })
    }

    fn wake(&self) -> watch::Receiver<u64> {
        self.0.wake()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn emit_assigns_increasing_seq_and_wakes_subscribers() {
        let log = EventLog::new(10);
        let mut wake = log.wake();
        assert_eq!(log.head(), 0);
        log.emit("a", json!({}), None);
        log.emit("b", json!({"x": 1}), Some("p_1"));
        assert_eq!(log.head(), 2);
        assert_eq!(*wake.borrow_and_update(), 2);
        let second = log.with_ring(|r| r.entry(2).map(|e| e.item.clone())).unwrap();
        assert_eq!(second.kind, "b");
        assert_eq!(second.pane_id.as_deref(), Some("p_1"));
    }

    #[test]
    fn the_ring_keeps_only_the_newest_events() {
        let log = EventLog::new(2);
        for i in 0..5 {
            log.emit("e", json!({"i": i}), None);
        }
        assert_eq!(log.with_ring(|r| (r.oldest(), r.head())), (4, 5));
    }
}
