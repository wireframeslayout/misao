//! 端末の raw mode と復元（Node 版の `attach/tty.ts`）。

use crate::error::CliError;
use crate::io::CliIo;

/// alt screen / マウス / bracketed paste / アプリカーソル / SGR / カーソル表示を既定へ戻す。
pub const TTY_RESET: &str = "\x1b[0m\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b[?25h\x1b[?9l\x1b[?1004l\x1b>";

/// stdin / stdout が端末でなければ usage エラー。
pub fn require_terminal(io: &CliIo) -> Result<(), CliError> {
    if io.stdin.is_tty && io.stdout.is_tty() {
        Ok(())
    } else {
        Err(CliError::usage("attach には端末（TTY）が必要です"))
    }
}

/// 端末を raw mode にして、どう終わっても（正常終了 / シグナル / 例外）元に戻す。戻す処理は 1 回だけ行う。
pub struct TtyGuard<'a> {
    io: &'a CliIo,
    is_active: bool,
}

impl<'a> TtyGuard<'a> {
    pub fn new(io: &'a CliIo) -> Self {
        Self { io, is_active: false }
    }

    /// raw mode に入る。端末でなければ usage エラー。
    pub fn enter(&mut self) -> Result<(), CliError> {
        if !self.io.stdin.is_tty {
            return Err(CliError::usage("attach には端末（TTY）が必要です"));
        }
        self.io.stdin.set_raw(true).map_err(|e| CliError::runtime(&e))?;
        self.is_active = true;
        Ok(())
    }

    /// raw mode を戻して端末を既定の状態にする。何度呼んでも 1 回だけ実行する。
    pub fn restore(&mut self) {
        if !self.is_active {
            return;
        }
        self.is_active = false;
        let _ = self.io.stdin.set_raw(false);
        self.io.stdout.write(TTY_RESET.as_bytes());
    }
}

impl Drop for TtyGuard<'_> {
    fn drop(&mut self) {
        self.restore();
    }
}
