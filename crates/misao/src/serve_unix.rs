//! `misao serve` の Unix 側: データディレクトリの作成（700）と、シグナルによる停止。

use std::future::Future;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use tokio::signal::unix::{Signal, SignalKind, signal};

pub fn create_data_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

pub struct Shutdown {
    int: Signal,
    term: Signal,
    hup: Signal,
}

impl Shutdown {
    pub fn register() -> io::Result<Self> {
        Ok(Self {
            int: signal(SignalKind::interrupt())?,
            term: signal(SignalKind::terminate())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    /// SIGINT / SIGTERM / SIGHUP のどれかが届いたら完了する。
    pub fn wait_future(mut self, _endpoint: &str) -> io::Result<impl Future<Output = ()>> {
        Ok(async move {
            tokio::select! {
                _ = self.int.recv() => {}
                _ = self.term.recv() => {}
                _ = self.hup.recv() => {}
            }
        })
    }
}
