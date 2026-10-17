//! キー指定（`C-^`、`d` など）の解釈と表示。Node 版の `config/keys.ts`。

const CTRL_TARGETS: &str = "@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_";
const MIN_PRINTABLE: u8 = 0x20;
const MAX_PRINTABLE: u8 = 0x7e;
const MAX_CONTROL_CODE: u8 = 0x1f;

/// アクションキー（prefix の次に押す）。
pub const ACTIONS: [&str; 4] = ["detach", "next", "prev", "list"];

/// attach のキー割り当て（入力バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBindings {
    pub prefix: u8,
    pub detach: u8,
    pub next: u8,
    pub prev: u8,
    pub list: u8,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            prefix: 0x1e,
            detach: b'd',
            next: b'n',
            prev: b'p',
            list: b'l',
        }
    }
}

/// `C-<ch>`（ch は `@A-Za-z[\]^_`）は制御コード、単一の印字可能 ASCII はそのまま 1 バイト。
/// `C-[` は ESC になり矢印キー等のエスケープシーケンスと区別できないため拒否する。
pub fn parse_key_spec(spec: &str) -> Result<u8, String> {
    if spec.starts_with("C-") {
        return parse_control_key(spec);
    }
    let mut bytes = spec.bytes();
    if let (Some(b), None) = (bytes.next(), bytes.next())
        && (MIN_PRINTABLE..=MAX_PRINTABLE).contains(&b)
    {
        return Ok(b);
    }
    Err(format!(
        "invalid key spec \"{spec}\": expected \"C-<char>\" or a single printable ASCII character"
    ))
}

/// prefix キー用。制御キー（`C-<char>`）のみを許す。
pub fn parse_prefix_key_spec(spec: &str) -> Result<u8, String> {
    if !spec.starts_with("C-") {
        return Err(format!(
            "invalid prefix key \"{spec}\": prefix must be a control key \"C-<char>\""
        ));
    }
    parse_control_key(spec)
}

fn parse_control_key(spec: &str) -> Result<u8, String> {
    let target = spec[2..].to_uppercase();
    if target == "[" {
        return Err(format!(
            "invalid key spec \"{spec}\": C-[ is ESC and cannot be used"
        ));
    }
    let mut chars = target.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if CTRL_TARGETS.contains(c) && c.is_ascii() => Ok((c as u8) & 0x1f),
        _ => Err(format!(
            "invalid key spec \"{spec}\": C-<char> requires one of {CTRL_TARGETS}"
        )),
    }
}

/// キーのバイト値を表示用の文字列にする。0x1e → `Ctrl-^`、印字可能文字はそのまま。
pub fn format_key_spec(code: u8) -> String {
    if code <= MAX_CONTROL_CODE {
        return format!("Ctrl-{}", char::from(code + 0x40));
    }
    if (MIN_PRINTABLE..=MAX_PRINTABLE).contains(&code) {
        return char::from(code).to_string();
    }
    format!("0x{code:02x}")
}

impl KeyBindings {
    pub fn action_key(&self, action: &str) -> Option<u8> {
        match action {
            "detach" => Some(self.detach),
            "next" => Some(self.next),
            "prev" => Some(self.prev),
            "list" => Some(self.list),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_and_plain_keys_parse() {
        assert_eq!(parse_key_spec("C-^"), Ok(0x1e));
        assert_eq!(parse_key_spec("C-a"), Ok(0x01));
        assert_eq!(parse_key_spec("C-@"), Ok(0));
        assert_eq!(parse_key_spec("d"), Ok(b'd'));
        assert_eq!(parse_key_spec("~"), Ok(b'~'));
    }

    #[test]
    fn invalid_specs_are_rejected_with_the_reason() {
        for bad in ["", "dd", "C-", "C-ab", "C-1", "é", "\u{7f}", "c-a"] {
            assert!(parse_key_spec(bad).is_err(), "{bad:?}");
        }
        assert!(parse_key_spec("C-[").is_err_and(|e| e.contains("ESC")));
    }

    #[test]
    fn prefix_must_be_a_control_key() {
        assert!(parse_prefix_key_spec("d").is_err_and(|e| e.contains("prefix must be a control key")));
        assert_eq!(parse_prefix_key_spec("C-a"), Ok(1));
    }

    #[test]
    fn keys_are_formatted_for_display() {
        assert_eq!(format_key_spec(0x1e), "Ctrl-^");
        assert_eq!(format_key_spec(0x01), "Ctrl-A");
        assert_eq!(format_key_spec(b'd'), "d");
    }
}
