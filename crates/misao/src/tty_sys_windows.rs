//! Windows のコンソール: raw mode は入力を VT シーケンスで受け（`ENABLE_VIRTUAL_TERMINAL_INPUT`）、
//! 出力の VT 解釈を有効にする。Windows に SIGWINCH は無いので、端末の大きさは一定間隔で調べて変化を知らせる。
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
    ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    SetConsoleMode,
};

use crate::io::{CliSignal, RawMode, Signals};

/// 端末の大きさを調べる間隔。
const SIZE_POLL: Duration = Duration::from_millis(250);

fn std_handle(which: u32) -> std::io::Result<HANDLE> {
    // SAFETY: 標準ハンドルを問い合わせるだけ。
    let handle = unsafe { GetStdHandle(which) };
    // INVALID_HANDLE_VALUE は -1。
    if handle.is_null() || handle as isize == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(handle)
}

fn get_mode(handle: HANDLE) -> std::io::Result<u32> {
    let mut mode = 0u32;
    // SAFETY: `mode` は書き込める。コンソールでなければ失敗する。
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(mode)
}

fn set_mode(handle: HANDLE, mode: u32) -> std::io::Result<()> {
    // SAFETY: コンソールのハンドルにモードを設定するだけ。
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// 端末の大きさ `(cols, rows)`（バッファではなく、見えている窓の大きさ）。
pub fn window_size() -> Option<(usize, usize)> {
    let handle = std_handle(STD_OUTPUT_HANDLE).ok()?;
    // SAFETY: 全ビットがゼロの構造体は有効で、成功したときだけ読む。
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `info` は書き込める構造体。
    if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
        return None;
    }
    let window = info.srWindow;
    let cols = i32::from(window.Right) - i32::from(window.Left) + 1;
    let rows = i32::from(window.Bottom) - i32::from(window.Top) + 1;
    Some((usize::try_from(cols).ok()?, usize::try_from(rows).ok()?))
}

/// raw mode に入る前の `(入力のモード, 出力のモード)`。パニックのフックからも戻せるよう、プロセスに 1 つだけ持つ。
static SAVED: Mutex<Option<(u32, u32)>> = Mutex::new(None);

fn restore_saved() -> bool {
    let saved = SAVED.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some((input, output)) = saved else {
        return false;
    };
    let restored_input = std_handle(STD_INPUT_HANDLE).and_then(|h| set_mode(h, input));
    let restored_output = std_handle(STD_OUTPUT_HANDLE).and_then(|h| set_mode(h, output));
    restored_input.is_ok() && restored_output.is_ok()
}

/// 標準入力の raw mode。入る前のモードを覚えて、戻すときに復元する。
#[derive(Default)]
pub struct StdinRawMode;

impl RawMode for StdinRawMode {
    fn set_raw(&self, raw: bool) -> std::io::Result<()> {
        if !raw {
            restore_saved();
            return Ok(());
        }
        let mut saved = SAVED.lock().unwrap_or_else(PoisonError::into_inner);
        if saved.is_some() {
            return Ok(());
        }
        let (input, output) = (std_handle(STD_INPUT_HANDLE)?, std_handle(STD_OUTPUT_HANDLE)?);
        let (input_mode, output_mode) = (get_mode(input)?, get_mode(output)?);
        // 行の編集・エコー・Ctrl+C の処理をやめ、キーは VT シーケンスで受ける。出力は VT を解釈させる。
        let raw_input = (input_mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        set_mode(input, raw_input)?;
        if let Err(e) = set_mode(
            output,
            output_mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        ) {
            let _ = set_mode(input, input_mode);
            return Err(e);
        }
        *saved = Some((input_mode, output_mode));
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

/// 受け手(セッション)が捨てられたら、次のシグナルを待たずに終わる(タスクを溜めない)。
macro_rules! forward {
    ($tx:expr, $which:expr, $stream:expr) => {
        if let Ok(mut stream) = $stream {
            let tx: UnboundedSender<CliSignal> = $tx.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        received = stream.recv() => {
                            if received.is_none() || tx.send($which).is_err() {
                                break;
                            }
                        }
                        () = tx.closed() => break,
                    }
                }
            });
        }
    };
}

/// SIGWINCH の代わり: 端末の大きさが変わったら知らせる。
fn watch_window_size(tx: UnboundedSender<CliSignal>) {
    tokio::spawn(async move {
        let mut last = window_size();
        loop {
            tokio::select! {
                () = tokio::time::sleep(SIZE_POLL) => {}
                () = tx.closed() => break,
            }
            let now = window_size();
            if now != last {
                last = now;
                if tx.send(CliSignal::WindowChange).is_err() {
                    break;
                }
            }
        }
    });
}

impl Signals for RealSignals {
    fn listen(&self, signals: &[CliSignal]) -> UnboundedReceiver<CliSignal> {
        let (tx, rx) = unbounded_channel();
        for &which in signals {
            match which {
                CliSignal::Interrupt => forward!(tx, which, ctrl_c()),
                CliSignal::Quit => forward!(tx, which, ctrl_break()),
                CliSignal::Hangup => forward!(tx, which, ctrl_close()),
                CliSignal::Terminate => forward!(tx, which, ctrl_shutdown()),
                CliSignal::WindowChange => watch_window_size(tx.clone()),
            }
        }
        rx
    }
}

/// Windows には passwd のログインシェルが無いので、cmd.exe を指す `%COMSPEC%`（PowerShell は既定にしない）。
pub fn login_shell(env: &HashMap<String, String>) -> Option<String> {
    crate::io::comspec_shell(env)
}
