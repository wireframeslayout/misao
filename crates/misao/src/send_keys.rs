//! キー名（Enter / Escape / Tab / C-c / Up など）を端末へ送るバイト列にする（Node 版の `send-keys.ts`）。

use crate::error::CliError;
use crate::keys::parse_key_spec;

fn named_key(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "enter" | "return" => b"\r",
        "escape" | "esc" => b"\x1b",
        "tab" => b"\t",
        "space" => b" ",
        "backspace" => b"\x7f",
        "delete" => b"\x1b[3~",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        _ => return None,
    })
}

/// 未知の名前は usage エラー。
pub fn encode_keys<S: AsRef<str>>(names: &[S]) -> Result<Vec<u8>, CliError> {
    let mut out = Vec::new();
    for name in names {
        let name = name.as_ref();
        let unknown = || CliError::usage(format!("不明なキー名です: {name}"));
        if let Some(bytes) = named_key(&name.to_lowercase()) {
            out.extend_from_slice(bytes);
        } else if name.len() >= 2 && name[..2].eq_ignore_ascii_case("c-") {
            let code = parse_key_spec(&format!("C-{}", &name[2..])).map_err(|_| unknown())?;
            out.push(code);
        } else {
            return Err(unknown());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_and_control_keys_are_case_insensitive() {
        assert_eq!(
            encode_keys(&["Enter", "escape", "C-c", "c-d", "Up"]).unwrap(),
            b"\r\x1b\x03\x04\x1b[A".to_vec()
        );
    }

    #[test]
    fn unknown_names_are_usage_errors() {
        for name in ["Bogus", "C-[", "c-"] {
            assert_eq!(encode_keys(&[name]).unwrap_err().exit_code(), 2, "{name}");
        }
    }
}
