//! ConPTY のカーソル位置の問い合わせ（DSR）への返事。
//!
//! portable-pty は ConPTY を `PSEUDOCONSOLE_INHERIT_CURSOR` で作る。この指定の ConPTY は、起動時に端末へカーソル位置を尋ねる
//! （`ESC[6n`）。**返事が来るまで子プロセスを動かさない**ので、端末の役をするデーモンが `ESC[<行>;<列>R` と返す必要がある
//! （返さないと、ペインの子は何も出力せず、`cmd.exe` も `ping` も始まらない）。実機で確かめた。
//! 文字列の照合だけなので、Unix でも単体テストできる（使うのは Windows だけ）。

/// 出力に含まれる `ESC[6n` を数える。チャンクの境目をまたぐ並びも数える。
#[derive(Debug, Default)]
pub struct CursorQueryScanner {
    /// `ESC [ 6 n` のうち、ここまでに一致した先頭の長さ。
    matched: usize,
}

const QUERY: &[u8] = b"\x1b[6n";

impl CursorQueryScanner {
    pub fn count(&mut self, data: &[u8]) -> usize {
        let mut found = 0;
        for &byte in data {
            if byte == QUERY[self.matched] {
                self.matched += 1;
                if self.matched == QUERY.len() {
                    found += 1;
                    self.matched = 0;
                }
            } else {
                // ESC はいつでも新しい並びの始まり。それ以外は最初からやり直す。
                self.matched = usize::from(byte == QUERY[0]);
            }
        }
        found
    }
}

/// `ESC[<行>;<列>R`（1 始まり）。`x` `y` は 0 始まりのカーソル位置。
pub fn cursor_report(x: usize, y: usize) -> Vec<u8> {
    format!("\x1b[{};{}R", y + 1, x + 1).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_queries_across_chunk_boundaries() {
        let mut scanner = CursorQueryScanner::default();
        assert_eq!(scanner.count(b"hello\x1b[6nworld\x1b[6n"), 2);
        let input = b"a\x1b[6nb\x1b[?25l\x1b[6n";
        let split: usize = input.iter().map(|b| scanner.count(std::slice::from_ref(b))).sum();
        assert_eq!(split, 2);
    }

    #[test]
    fn near_misses_are_not_queries() {
        let mut scanner = CursorQueryScanner::default();
        assert_eq!(scanner.count(b"\x1b[5n\x1b[6c\x1b[?6n\x1b6n\x1b\x1b[6"), 0);
        // 直前の ESC が次の並びの始まりになる: ESC ESC [ 6 n
        assert_eq!(scanner.count(b"n"), 1);
    }

    #[test]
    fn the_report_is_one_based() {
        assert_eq!(cursor_report(0, 0), b"\x1b[1;1R");
        assert_eq!(cursor_report(41, 3), b"\x1b[4;42R");
    }
}
