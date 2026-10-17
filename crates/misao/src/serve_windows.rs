//! `misao serve` の Windows 側: データディレクトリの作成と、コンソールの終了または `misao stop` による停止。

use std::future::Future;
use std::io;
use std::path::Path;

use misao_daemon::StopEvent;
use tokio::signal::windows::{
    CtrlBreak, CtrlC, CtrlClose, CtrlLogoff, CtrlShutdown, ctrl_break, ctrl_c, ctrl_close, ctrl_logoff,
    ctrl_shutdown,
};

/// Windows のディレクトリは、親の ACL を継承する（`ensure_data_dir` が起動時に検証する）。
pub fn create_data_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

pub struct Shutdown {
    ctrl_c: CtrlC,
    ctrl_break: CtrlBreak,
    ctrl_close: CtrlClose,
    ctrl_logoff: CtrlLogoff,
    ctrl_shutdown: CtrlShutdown,
}

impl Shutdown {
    pub fn register() -> io::Result<Self> {
        Ok(Self {
            ctrl_c: ctrl_c()?,
            ctrl_break: ctrl_break()?,
            ctrl_close: ctrl_close()?,
            ctrl_logoff: ctrl_logoff()?,
            ctrl_shutdown: ctrl_shutdown()?,
        })
    }

    /// Ctrl+C / Ctrl+Break / コンソールを閉じる / ログオフ / シャットダウン、または `misao stop` で完了する。
    /// `misao stop` の窓口（名前付きイベント）は、接続口を確保した後（デーモンの起動後）に作る。
    pub fn wait_future(mut self, endpoint: &str) -> io::Result<impl Future<Output = ()>> {
        let stop = StopEvent::create(endpoint)?;
        Ok(async move {
            tokio::select! {
                _ = self.ctrl_c.recv() => {}
                _ = self.ctrl_break.recv() => {}
                _ = self.ctrl_close.recv() => {}
                _ = self.ctrl_logoff.recv() => {}
                _ = self.ctrl_shutdown.recv() => {}
                () = stop.stopped() => {}
            }
        })
    }
}
