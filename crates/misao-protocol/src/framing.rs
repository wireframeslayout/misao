use serde::Serialize;

/// 1 行の上限（`DEFAULT_MAX_LINE_BYTES` in `packages/protocol/src/framing.ts`）。
pub const DEFAULT_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("NDJSON line exceeds {0} bytes")]
pub struct LineError(pub usize);

/// NDJSON: 1 行 1 JSON。末尾に改行を付ける。
pub fn encode_message<T: Serialize>(msg: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = serde_json::to_vec(msg)?;
    out.push(b'\n');
    Ok(out)
}

/// チャンク境界をまたぐ行分割器。バイト列のまま行を組み立ててから UTF-8 で復号するので、
/// マルチバイト文字がチャンク境界で切れても壊れない。空白だけの行は捨てる。
#[derive(Debug)]
pub struct LineSplitter {
    pending: Vec<u8>,
    max_line_bytes: usize,
}

impl Default for LineSplitter {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LINE_BYTES)
    }
}

impl LineSplitter {
    pub fn new(max_line_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            max_line_bytes,
        }
    }

    /// 上限を超えたらエラー（呼び出し側は接続を切る）。不正な UTF-8 は U+FFFD に置き換える（Node の復号と同じ）。
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, LineError> {
        let mut lines = Vec::new();
        let mut rest = chunk;
        while let Some(idx) = rest.iter().position(|&b| b == b'\n') {
            self.append(&rest[..idx])?;
            let line = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            if !line.trim().is_empty() {
                lines.push(line);
            }
            rest = &rest[idx + 1..];
        }
        self.append(rest)?;
        Ok(lines)
    }

    fn append(&mut self, part: &[u8]) -> Result<(), LineError> {
        if self.pending.len() + part.len() > self.max_line_bytes {
            self.pending.clear();
            return Err(LineError(self.max_line_bytes));
        }
        self.pending.extend_from_slice(part);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_across_chunks_and_multibyte_boundaries() {
        let mut s = LineSplitter::default();
        let bytes = "{\"a\":\"あ\"}\n{\"b\":1}\n".as_bytes();
        let (head, tail) = bytes.split_at(7); // 「あ」の途中
        assert!(s.push(head).unwrap().is_empty());
        assert_eq!(s.push(tail).unwrap(), vec!["{\"a\":\"あ\"}", "{\"b\":1}"]);
    }

    #[test]
    fn drops_blank_lines() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"\n  \nx\n").unwrap(), vec!["x"]);
    }

    #[test]
    fn rejects_oversized_line() {
        let mut s = LineSplitter::new(4);
        assert_eq!(s.push(b"abcde"), Err(LineError(4)));
    }

    #[test]
    fn encodes_with_trailing_newline() {
        assert_eq!(
            encode_message(&serde_json::json!({"a": 1})).unwrap(),
            b"{\"a\":1}\n"
        );
    }
}
