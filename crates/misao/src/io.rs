//! プロセスとの境界（Node 版の `cli-io.ts`）。コマンドは `process` を直接触らず、これを受け取る（テストで差し替える）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(all(test, unix))]
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// attach / tail / serve が受け取るシグナル。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CliSignal {
    Interrupt,
    Terminate,
    Hangup,
    Quit,
    WindowChange,
}

impl CliSignal {
    /// attach と一覧で受けて終了する（終了コード 1）シグナル。
    pub const TERMINATION: [Self; 4] = [Self::Terminate, Self::Hangup, Self::Interrupt, Self::Quit];
}

/// 出力先（stdout / stderr）。
pub trait Sink: Send + Sync {
    fn write(&self, bytes: &[u8]);
    fn is_tty(&self) -> bool;
    /// 端末の大きさ `(cols, rows)`。端末でなければ None。0 を報告する端末もある（呼び出し側が無視する）。
    fn size(&self) -> Option<(usize, usize)>;
}

/// 端末の入力モード（raw mode）。
pub trait RawMode: Send + Sync {
    fn set_raw(&self, raw: bool) -> std::io::Result<()>;
}

/// シグナルの購読。購読した間だけ、そのシグナルを既定の動作の代わりに受ける。
pub trait Signals: Send + Sync {
    fn listen(&self, signals: &[CliSignal]) -> UnboundedReceiver<CliSignal>;
}

type Starter = Box<dyn FnOnce() -> UnboundedReceiver<Vec<u8>> + Send>;

/// 標準入力。チャンクの列として受け取り、行の読み取りでは余りを次に持ち越す。
pub struct Stdin {
    pub is_tty: bool,
    start: Mutex<Option<Starter>>,
    rx: tokio::sync::Mutex<Option<UnboundedReceiver<Vec<u8>>>>,
    pending: Mutex<Vec<u8>>,
    raw: Arc<dyn RawMode>,
}

impl Stdin {
    pub fn new(is_tty: bool, raw: Arc<dyn RawMode>, start: Starter) -> Self {
        Self {
            is_tty,
            start: Mutex::new(Some(start)),
            rx: tokio::sync::Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            raw,
        }
    }

    /// テスト用: 送り口から入力を流せる標準入力。送り口を落とすと EOF。
    #[cfg(all(test, unix))]
    pub fn channel(is_tty: bool, raw: Arc<dyn RawMode>) -> (Self, UnboundedSender<Vec<u8>>) {
        let (tx, rx) = unbounded_channel();
        let stdin = Self::new(is_tty, raw, Box::new(move || rx));
        (stdin, tx)
    }

    pub fn set_raw(&self, raw: bool) -> std::io::Result<()> {
        self.raw.set_raw(raw)
    }

    fn take_pending(&self) -> Vec<u8> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// 次のチャンク。持ち越しがあれば先にそれを返す。入力が閉じたら None。
    pub async fn recv(&self) -> Option<Vec<u8>> {
        let pending = self.take_pending();
        if !pending.is_empty() {
            return Some(pending);
        }
        self.recv_chunk().await
    }

    async fn recv_chunk(&self) -> Option<Vec<u8>> {
        let mut rx = self.rx.lock().await;
        if rx.is_none() {
            let start = self.start.lock().unwrap_or_else(PoisonError::into_inner).take();
            *rx = start.map(|f| f());
        }
        rx.as_mut()?.recv().await
    }

    /// 1 行読む（改行は除く）。入力が閉じたとき、改行なしの残りがあればそれを、無ければ None。
    pub async fn read_line(&self) -> Option<String> {
        loop {
            {
                let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(at) = pending.iter().position(|&b| b == b'\n') {
                    let rest = pending.split_off(at + 1);
                    let line = std::mem::replace(&mut *pending, rest);
                    let text = String::from_utf8_lossy(&line[..at]).into_owned();
                    return Some(text.strip_suffix('\r').map_or(text.clone(), str::to_string));
                }
            }
            match self.recv_chunk().await {
                Some(chunk) => self
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(&chunk),
                None => {
                    let rest = self.take_pending();
                    return (!rest.is_empty()).then(|| String::from_utf8_lossy(&rest).into_owned());
                }
            }
        }
    }

    /// 入力が閉じるまで全部読む。
    pub async fn read_all(&self) -> Vec<u8> {
        let mut all = self.take_pending();
        while let Some(chunk) = self.recv_chunk().await {
            all.extend_from_slice(&chunk);
        }
        all
    }
}

pub struct CliIo {
    pub stdout: Arc<dyn Sink>,
    pub stderr: Arc<dyn Sink>,
    pub stdin: Stdin,
    pub env: HashMap<String, String>,
    pub home_dir: String,
    pub cwd: PathBuf,
    pub pid: u32,
    /// ログインシェル。取れない環境では None。
    pub shell: Option<String>,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub signals: Arc<dyn Signals>,
}

impl CliIo {
    pub fn now_ms(&self) -> i64 {
        (self.now)()
    }

    pub fn write_line(sink: &dyn Sink, text: &str) {
        sink.write(format!("{text}\n").as_bytes());
    }

    pub fn out_line(&self, text: &str) {
        Self::write_line(self.stdout.as_ref(), text);
    }

    pub fn err_line(&self, text: &str) {
        Self::write_line(self.stderr.as_ref(), text);
    }

    /// 端末の大きさ。端末が 0 や不明を報告したら None（ペインのサイズは変えない）。
    pub fn terminal_size(&self) -> Option<(usize, usize)> {
        self.stdout.size().filter(|&(cols, rows)| cols > 0 && rows > 0)
    }
}

/// 実プロセスの標準出力 / 標準エラー。
struct StdSink {
    is_err: bool,
    is_tty: bool,
}

impl Sink for StdSink {
    fn write(&self, bytes: &[u8]) {
        // 書けない（パイプが閉じた等）ときは黙って捨てる。Node の stream も EPIPE では何も出さない。
        if self.is_err {
            let mut e = std::io::stderr().lock();
            let _ = e.write_all(bytes).and_then(|()| e.flush());
        } else {
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(bytes).and_then(|()| o.flush());
        }
    }

    fn is_tty(&self) -> bool {
        self.is_tty
    }

    fn size(&self) -> Option<(usize, usize)> {
        if !self.is_tty {
            return None;
        }
        crate::tty_sys::window_size()
    }
}

fn env_map() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// 実プロセスの `CliIo`。
pub fn real_io() -> CliIo {
    use std::io::IsTerminal;
    let stdin_tty = std::io::stdin().is_terminal();
    crate::tty_sys::install_panic_restore(crate::attach::tty::TTY_RESET);
    let raw: Arc<dyn RawMode> = Arc::new(crate::tty_sys::StdinRawMode);
    let start: Starter = Box::new(|| {
        let (tx, rx) = unbounded_channel();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = stdin.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        rx
    });
    CliIo {
        stdout: Arc::new(StdSink {
            is_err: false,
            is_tty: std::io::stdout().is_terminal(),
        }),
        stderr: Arc::new(StdSink {
            is_err: true,
            is_tty: std::io::stderr().is_terminal(),
        }),
        stdin: Stdin::new(stdin_tty, raw, start),
        env: env_map(),
        home_dir: std::env::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default(),
        cwd: std::env::current_dir().unwrap_or_default(),
        pid: std::process::id(),
        shell: crate::tty_sys::login_shell(&env_map()),
        now: Arc::new(now_ms),
        signals: Arc::new(crate::tty_sys::RealSignals),
    }
}

/// Windows には passwd のログインシェルが無いので、cmd.exe を指す `%COMSPEC%`（PowerShell は既定にしない）。無ければ None。
/// シェルが要るのは new / 一覧の n だけで、そこで usage エラーにする。
#[cfg_attr(unix, allow(dead_code))]
pub fn comspec_shell(env: &HashMap<String, String>) -> Option<String> {
    env.get("COMSPEC").filter(|v| !v.is_empty()).cloned()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(all(test, unix))]
pub mod testing {
    //! テスト用の `CliIo`（Node 版の `test/helpers/io.ts`）。

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
        pub stdin: UnboundedSender<Vec<u8>>,
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
                stdin: sender,
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
            let _ = self.stdin.send(data.to_vec());
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

    use std::time::Duration;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_uses_comspec_and_nothing_else() {
        let env = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        assert_eq!(
            comspec_shell(&env(&[("COMSPEC", "C:\\Windows\\System32\\cmd.exe")])).as_deref(),
            Some("C:\\Windows\\System32\\cmd.exe")
        );
        assert_eq!(comspec_shell(&env(&[])), None);
        assert_eq!(comspec_shell(&env(&[("COMSPEC", "")])), None);
    }
}
