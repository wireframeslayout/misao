/// 行ストリームのリングは 1 行ごとに文字数 + このオーバーヘッドで数える（空行ばかりでもエントリ数が膨らまないように）。
/// Node 版の `LINE_ENTRY_OVERHEAD`。
pub const LINE_ENTRY_OVERHEAD: usize = 64;

/// pty へ書く入力の待ち行列の上限（バイト）。子が読まないままこれを超えて溜めない。
/// 待ち行列が空のときの 1 回の入力は、これより大きくても受ける（上限は 1 行 8 MiB の NDJSON が決める）。
pub const INPUT_QUEUE_BYTES: usize = 4 * 1024 * 1024;

/// デーモンの上限値。既定は Node 版の `DEFAULT_DAEMON_LIMITS` と同じ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// headless 端末のスクロールバック行数（画面は第 3 段）。
    pub scrollback: usize,
    /// 生出力リングの上限バイト数（pane ごと）。
    pub raw_ring_bytes: usize,
    /// 行ストリームリングの上限バイト数（pane ごと）。
    pub lines_ring_bytes: usize,
    /// イベントリングの件数。
    pub events: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            scrollback: 5000,
            raw_ring_bytes: 1024 * 1024,
            lines_ring_bytes: 4 * 1024 * 1024,
            events: 1000,
        }
    }
}
