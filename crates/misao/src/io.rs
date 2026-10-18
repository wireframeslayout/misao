//! プロセスとの境界（Node 版の `cli-io.ts`）。コマンドは `process` を直接触らず、これを受け取る（テストで差し替える）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

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

#[path = "io_stdin.rs"]
mod stdin_impl;
use stdin_impl::Starter;
pub use stdin_impl::Stdin;

#[cfg(all(test, unix))]
#[path = "io_testing.rs"]
pub mod testing;

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

/// 環境変数の表。Windows は名前の大文字小文字を区別しない（`ComSpec` と `COMSPEC` は同じ）ので、大文字にそろえて引けるようにする。
fn env_map() -> HashMap<String, String> {
    env_map_from(std::env::vars(), cfg!(windows))
}

fn env_map_from(
    vars: impl IntoIterator<Item = (String, String)>,
    is_case_insensitive: bool,
) -> HashMap<String, String> {
    vars.into_iter()
        .map(|(key, value)| {
            (
                if is_case_insensitive {
                    key.to_uppercase()
                } else {
                    key
                },
                value,
            )
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_environment_names_are_matched_case_insensitively() {
        let vars = || [("ComSpec".to_string(), "C:\\cmd.exe".to_string())];
        let windows = env_map_from(vars(), true);
        assert_eq!(comspec_shell(&windows).as_deref(), Some("C:\\cmd.exe"));
        assert_eq!(comspec_shell(&env_map_from(vars(), false)), None);
    }

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
