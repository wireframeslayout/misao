//! 停止イベントの名前。接続口の名前の表記揺れ（区切りの `/` と `\`、大文字小文字、`\\.\pipe\` の接頭辞）を除き、
//! デーモンの起動世代（`server.info` の epoch）を含める。epoch が違う（問い合わせたのとは別の）デーモンには作用しない。
//! 文字列だけなので Unix でも単体テストできる（使うのは Windows だけ）。

#![cfg_attr(not(windows), allow(dead_code))]

use sha2::{Digest, Sha256};

const PIPE_PREFIX: &str = "\\\\.\\pipe\\";

/// 区切りを `\` にそろえて小文字にし、`\\.\pipe\` の接頭辞を除いたパイプ名。
fn normalize(endpoint: &str) -> String {
    let lowered = endpoint.replace('/', "\\").to_lowercase();
    lowered
        .strip_prefix(PIPE_PREFIX)
        .map_or(lowered.clone(), str::to_string)
}

/// `Global\misao-stop-<sha256(正規化した名前 + LF + epoch) の先頭 16 桁>`。
pub fn stop_event_name(endpoint: &str, epoch: &str) -> String {
    let digest = Sha256::digest(format!("{}\n{epoch}", normalize(endpoint)).as_bytes());
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("Global\\misao-stop-{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spelling_differences_of_the_same_pipe_give_the_same_name() {
        let epoch = "01M5630DCXG0MA9GFXR170PDCS";
        let a = stop_event_name("\\\\.\\pipe\\Misao-Test", epoch);
        assert_eq!(a, stop_event_name("//./PIPE/misao-test", epoch));
        assert_eq!(a, stop_event_name("\\\\.\\pipe\\misao-test", epoch));
        assert!(a.starts_with("Global\\misao-stop-") && a.len() == "Global\\misao-stop-".len() + 16);
    }

    #[test]
    fn another_pipe_or_another_epoch_gives_another_name() {
        let base = stop_event_name("\\\\.\\pipe\\a", "E1");
        assert_ne!(base, stop_event_name("\\\\.\\pipe\\b", "E1"));
        assert_ne!(base, stop_event_name("\\\\.\\pipe\\a", "E2"));
    }
}
