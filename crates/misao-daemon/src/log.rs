use std::sync::Arc;

use time::OffsetDateTime;
use time::macros::format_description;

/// ログの閾値（error < warn < info < debug）。`misao.json` の `logLevel` と同じ名前。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    pub const DEFAULT: Self = Self::Info;

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }
}

type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// level より詳細な出力を捨てて、残りを sink へ渡す。
#[derive(Clone)]
pub struct Logger {
    level: LogLevel,
    sink: Sink,
}

/// `2026-10-17T12:00:00.123Z`（Node 版の `new Date().toISOString()` と同じ形）。
pub fn now_iso() -> String {
    const FORMAT: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    OffsetDateTime::now_utc().format(FORMAT).unwrap_or_default()
}

impl Logger {
    pub fn new(level: LogLevel, sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            level,
            sink: Arc::new(sink),
        }
    }

    /// stderr に `[misao <ISO 時刻>] <msg>` の形式で書く（Node 版の `misao serve` と同じ）。
    pub fn stderr(level: LogLevel) -> Self {
        Self::new(level, |msg| eprintln!("[misao {}] {msg}", now_iso()))
    }

    fn at(&self, target: LogLevel, msg: &str) {
        if target <= self.level {
            (self.sink)(msg);
        }
    }

    pub fn error(&self, msg: &str) {
        self.at(LogLevel::Error, msg);
    }

    pub fn warn(&self, msg: &str) {
        self.at(LogLevel::Warn, msg);
    }

    pub fn info(&self, msg: &str) {
        self.at(LogLevel::Info, msg);
    }

    pub fn debug(&self, msg: &str) {
        self.at(LogLevel::Debug, msg);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn drops_more_detailed_output() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let log = Logger::new(LogLevel::Warn, move |m| sink.lock().unwrap().push(m.to_string()));
        log.error("e");
        log.warn("w");
        log.info("i");
        log.debug("d");
        assert_eq!(*seen.lock().unwrap(), vec!["e", "w"]);
    }

    #[test]
    fn iso_timestamp_has_millis_and_z() {
        let s = now_iso();
        assert_eq!(s.len(), 24, "{s}");
        assert!(s.ends_with('Z'));
    }
}
