//! OS 依存の端末まわり（raw mode、端末の大きさ、シグナル、ログインシェル）。Unix 以外は未対応（#43）。

#[cfg(unix)]
mod imp {
    use std::collections::HashMap;
    use std::sync::{Mutex, PoisonError};

    use rustix::termios::{
        ControlModes, InputModes, LocalModes, OptionalActions, OutputModes, SpecialCodeIndex, Termios,
        tcgetattr, tcgetwinsize, tcsetattr,
    };
    use tokio::signal::unix::{SignalKind, signal};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use crate::io::{CliSignal, RawMode, Signals};

    /// 端末の大きさ `(cols, rows)`。
    pub fn window_size() -> Option<(usize, usize)> {
        let size = tcgetwinsize(rustix::stdio::stdout()).ok()?;
        Some((usize::from(size.ws_col), usize::from(size.ws_row)))
    }

    /// raw mode に入る前の設定。パニックのフックからも戻せるよう、プロセスに 1 つだけ持つ。
    static SAVED: Mutex<Option<Termios>> = Mutex::new(None);

    fn restore_saved() -> bool {
        let mut saved = SAVED.lock().unwrap_or_else(PoisonError::into_inner);
        match saved.take() {
            Some(original) => tcsetattr(rustix::stdio::stdin(), OptionalActions::Flush, &original).is_ok(),
            None => false,
        }
    }

    /// 標準入力の raw mode。入る前の設定を覚えて、戻すときに復元する。
    #[derive(Default)]
    pub struct StdinRawMode;

    impl RawMode for StdinRawMode {
        fn set_raw(&self, raw: bool) -> std::io::Result<()> {
            let fd = rustix::stdio::stdin();
            if !raw {
                restore_saved();
                return Ok(());
            }
            let mut saved = SAVED.lock().unwrap_or_else(PoisonError::into_inner);
            if saved.is_some() {
                return Ok(());
            }
            let original = tcgetattr(fd)?;
            let mut t = original.clone();
            // libuv の UV_TTY_MODE_RAW と同じ（出力の改行変換は残し、シグナル文字は入力として扱う）。
            t.input_modes.remove(
                InputModes::BRKINT
                    | InputModes::ICRNL
                    | InputModes::INPCK
                    | InputModes::ISTRIP
                    | InputModes::IXON,
            );
            t.output_modes.insert(OutputModes::ONLCR);
            t.control_modes.insert(ControlModes::CS8);
            t.local_modes
                .remove(LocalModes::ECHO | LocalModes::ICANON | LocalModes::IEXTEN | LocalModes::ISIG);
            t.special_codes[SpecialCodeIndex::VMIN] = 1;
            t.special_codes[SpecialCodeIndex::VTIME] = 0;
            tcsetattr(fd, OptionalActions::Flush, &t)?;
            *saved = Some(original);
            Ok(())
        }
    }

    /// 想定外のパニックでも、raw mode に入っていれば端末を戻してからエラーを出して終了コード 1 で終わる
    /// （Node 版の `uncaughtException` の扱いと同じ）。
    pub fn install_panic_restore(reset: &'static str) {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if restore_saved() {
                let mut out = std::io::stdout().lock();
                let _ = std::io::Write::write_all(&mut out, reset.as_bytes());
                let _ = std::io::Write::flush(&mut out);
                let message = info
                    .payload()
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                let mut err = std::io::stderr().lock();
                let _ = std::io::Write::write_all(
                    &mut err,
                    format!("[misao] 予期しないエラー: {message}\n").as_bytes(),
                );
                std::process::exit(1);
            }
            previous(info);
        }));
    }

    pub struct RealSignals;

    fn kind(signal: CliSignal) -> SignalKind {
        match signal {
            CliSignal::Interrupt => SignalKind::interrupt(),
            CliSignal::Terminate => SignalKind::terminate(),
            CliSignal::Hangup => SignalKind::hangup(),
            CliSignal::Quit => SignalKind::quit(),
            CliSignal::WindowChange => SignalKind::window_change(),
        }
    }

    impl Signals for RealSignals {
        fn listen(&self, signals: &[CliSignal]) -> UnboundedReceiver<CliSignal> {
            let (tx, rx) = unbounded_channel();
            for &which in signals {
                let Ok(mut stream) = signal(kind(which)) else {
                    continue;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    while stream.recv().await.is_some() {
                        if tx.send(which).is_err() {
                            break;
                        }
                    }
                });
            }
            rx
        }
    }

    /// passwd のログインシェル。取れなければ None。
    pub fn login_shell(_env: &HashMap<String, String>) -> Option<String> {
        nix::unistd::User::from_uid(nix::unistd::getuid())
            .ok()
            .flatten()
            .map(|user| user.shell.to_string_lossy().into_owned())
            .filter(|shell| !shell.is_empty())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::collections::HashMap;

    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use crate::io::{CliSignal, RawMode, Signals};

    pub fn window_size() -> Option<(usize, usize)> {
        None
    }

    #[derive(Default)]
    pub struct StdinRawMode;

    pub fn install_panic_restore(_reset: &'static str) {}

    impl RawMode for StdinRawMode {
        fn set_raw(&self, _raw: bool) -> std::io::Result<()> {
            Err(std::io::Error::other(
                "raw mode is not supported on this platform yet",
            ))
        }
    }

    pub struct RealSignals;

    impl Signals for RealSignals {
        fn listen(&self, _signals: &[CliSignal]) -> UnboundedReceiver<CliSignal> {
            unbounded_channel().1
        }
    }

    pub fn login_shell(env: &HashMap<String, String>) -> Option<String> {
        crate::io::comspec_shell(env)
    }
}

pub use imp::{RealSignals, StdinRawMode, install_panic_restore, login_shell, window_size};
