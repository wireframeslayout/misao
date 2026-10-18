//! ConPTY のカーソル位置の問い合わせ（DSR）への返事。
//!
//! portable-pty は ConPTY を `PSEUDOCONSOLE_INHERIT_CURSOR` で作る。この指定の ConPTY は、起動時に端末へカーソル位置を尋ねる
//! （`ESC[6n`）。**返事が来るまで子プロセスを動かさない**ので、端末の役をするデーモンが `ESC[<行>;<列>R` と返す必要がある
//! （返さないと、ペインの子は何も出力せず、`cmd.exe` も `ping` も始まらない）。実機で確かめた。
//! 文字列の照合だけなので、Unix でも単体テストできる（使うのは Windows だけ）。

/// 出力から `ESC[6n` を取り除く。チャンクの境目をまたぐ並びも扱う（`ESC` `[` `6` で終わるチャンクの末尾は、次のチャンクまで持ち越す）。
#[derive(Debug, Default)]
pub struct CursorQueryScanner {
    /// 前のチャンクの末尾にあった、`ESC [ 6 n` の途中までの先頭（最大 3 バイト）。
    held: Vec<u8>,
}

const QUERY: &[u8] = b"\x1b[6n";

impl CursorQueryScanner {
    /// 持ち越している `ESC` `ESC[` `ESC[6` を取り出す（終了時に、通常の出力として流すため）。
    pub fn take_held(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }

    /// 問い合わせを除いた出力と、除いた問い合わせの数を返す。
    pub fn extract(&mut self, data: &[u8]) -> (Vec<u8>, usize) {
        let mut input = std::mem::take(&mut self.held);
        input.extend_from_slice(data);
        let mut clean = Vec::with_capacity(input.len());
        let mut found = 0;
        let mut index = 0;
        while index < input.len() {
            let rest = &input[index..];
            if rest.starts_with(QUERY) {
                found += 1;
                index += QUERY.len();
            } else if rest.len() < QUERY.len() && QUERY.starts_with(rest) {
                self.held = rest.to_vec();
                break;
            } else {
                clean.push(input[index]);
                index += 1;
            }
        }
        (clean, found)
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
    fn queries_are_removed_from_the_output_and_counted() {
        let mut scanner = CursorQueryScanner::default();
        assert_eq!(
            scanner.extract(b"hello\x1b[6nworld\x1b[6n"),
            (b"helloworld".to_vec(), 2)
        );
        assert_eq!(
            scanner.extract(b"no query \x1b[31m"),
            (b"no query \x1b[31m".to_vec(), 0)
        );
    }

    #[test]
    fn a_query_split_at_every_byte_is_removed_and_nothing_else_is_lost() {
        let mut scanner = CursorQueryScanner::default();
        let input = b"a\x1b[6nb\x1b[?25l\x1b[6nc";
        let (mut clean, mut found) = (Vec::new(), 0);
        for byte in input {
            let (part, n) = scanner.extract(std::slice::from_ref(byte));
            clean.extend(part);
            found += n;
        }
        assert_eq!((clean, found), (b"ab\x1b[?25lc".to_vec(), 2));
    }

    #[test]
    fn near_misses_are_kept_in_order() {
        let mut scanner = CursorQueryScanner::default();
        let (mut clean, mut found) = scanner.extract(b"\x1b[5n\x1b[6c\x1b[?6n\x1b6n\x1b\x1b[6");
        // 末尾の `ESC [ 6` は問い合わせかもしれないので持ち越す
        assert_eq!(found, 0);
        let (rest, n) = scanner.extract(b"n!");
        clean.extend(rest);
        found += n;
        assert_eq!((clean, found), (b"\x1b[5n\x1b[6c\x1b[?6n\x1b6n\x1b!".to_vec(), 1));
    }

    #[test]
    fn an_unfinished_prefix_at_the_end_can_be_taken_back() {
        for prefix in [&b"\x1b"[..], b"\x1b[", b"\x1b[6"] {
            let mut scanner = CursorQueryScanner::default();
            let (clean, found) = scanner.extract(&[b"ab".as_slice(), prefix].concat());
            assert_eq!((clean, found), (b"ab".to_vec(), 0));
            assert_eq!(scanner.take_held(), prefix);
            assert_eq!(scanner.take_held(), b"", "taken once");
        }
    }

    #[test]
    fn the_report_is_one_based() {
        assert_eq!(cursor_report(0, 0), b"\x1b[1;1R");
        assert_eq!(cursor_report(41, 3), b"\x1b[4;42R");
    }
}
