//! テスト用の `CliIo`（Node 版の `test/helpers/io.ts`）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::*;

#[derive(Default)]
pub struct Capture {
    bytes: Mutex<Vec<u8>>,
    is_tty: bool,
    size: Mutex<Option<(usize, usize)>>,
}

impl Capture {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

impl Sink for Capture {
    fn write(&self, bytes: &[u8]) {
        self.bytes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
    }

    fn is_tty(&self) -> bool {
        self.is_tty
    }

    fn size(&self) -> Option<(usize, usize)> {
        *self.size.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Default)]
pub struct RawModes(pub Mutex<Vec<bool>>);

impl RawMode for RawModes {
    fn set_raw(&self, raw: bool) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(raw);
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeSignals(Mutex<Vec<(Vec<CliSignal>, UnboundedSender<CliSignal>)>>);

impl Signals for FakeSignals {
    fn listen(&self, signals: &[CliSignal]) -> UnboundedReceiver<CliSignal> {
        let (tx, rx) = unbounded_channel();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((signals.to_vec(), tx));
        rx
    }
}

impl FakeSignals {
    pub fn emit(&self, signal: CliSignal) {
        for (wanted, tx) in self.0.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            if wanted.contains(&signal) {
                let _ = tx.send(signal);
            }
        }
    }
}

pub struct TestIo {
    pub io: CliIo,
    pub out: Arc<Capture>,
    pub err: Arc<Capture>,
    stdin: Mutex<Option<UnboundedSender<Vec<u8>>>>,
    pub raw_modes: Arc<RawModes>,
    pub signals: Arc<FakeSignals>,
}

#[derive(Default)]
pub struct TestIoOptions {
    pub is_tty: bool,
    pub size: Option<(usize, usize)>,
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub home: Option<String>,
    pub env: HashMap<String, String>,
}

impl TestIo {
    pub fn new(options: TestIoOptions) -> Self {
        let capture = |tty| {
            Arc::new(Capture {
                is_tty: tty,
                size: Mutex::new(options.size),
                ..Capture::default()
            })
        };
        let (out, err) = (capture(options.is_tty), capture(options.is_tty));
        let raw_modes = Arc::new(RawModes::default());
        let signals = Arc::new(FakeSignals::default());
        let (stdin, sender) = Stdin::channel(options.is_tty, raw_modes.clone());
        let io = CliIo {
            stdout: out.clone(),
            stderr: err.clone(),
            stdin,
            env: options.env,
            home_dir: options.home.unwrap_or_else(|| "/home/test".to_string()),
            cwd: options.cwd.unwrap_or_else(|| PathBuf::from("/work/test")),
            pid: 4242,
            shell: options.shell,
            now: Arc::new(now_ms),
            signals: signals.clone(),
        };
        Self {
            io,
            out,
            err,
            stdin: Mutex::new(Some(sender)),
            raw_modes,
            signals,
        }
    }

    pub fn raw_modes(&self) -> Vec<bool> {
        self.raw_modes
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_size(&self, cols: usize, rows: usize) {
        for capture in [&self.out, &self.err] {
            *capture.size.lock().unwrap_or_else(PoisonError::into_inner) = Some((cols, rows));
        }
    }

    pub fn type_text(&self, data: &[u8]) {
        if let Some(sender) = self.stdin.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            let _ = sender.send(data.to_vec());
        }
    }

    /// 標準入力を閉じる(EOF)。
    pub fn close_stdin(&self) {
        self.stdin.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

/// 条件が満たされるまで待つ（10 秒で失敗）。
pub async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
