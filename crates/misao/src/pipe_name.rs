//! Windows の接続口（名前付きパイプ）の名前の解決。`packages/sdk-node/src/socket-path.ts` の win32 側と同じ規則。
//! 既定の名前は `\\.\pipe\misao-<user>-<sha256(小文字にした misao ディレクトリ)の先頭 16 桁>`。
//! 文字列だけで解決するので、Windows でなくても単体テストできる（実際に使うのは Windows だけ）。

// 名前の解決は文字列だけなので Unix でもテストするが、本体で使うのは Windows だけ。
#![cfg_attr(not(windows), allow(dead_code))]

use sha2::{Digest, Sha256};

use crate::socket_path::{PathEnv, PathError};

/// 名前付きパイプの名前空間の接頭辞。
pub const PIPE_PREFIX: &str = "\\\\.\\pipe\\";
/// 接頭辞を含めたパイプ名の長さの上限（文字数）。
pub const MAX_PIPE_NAME_LENGTH: usize = 256;
const DEFAULT_DIR_NAME: &str = ".misao";
const USER_MAX_LENGTH: usize = 32;
const HASH_LENGTH: usize = 16;

fn is_set(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

/// `\\.\pipe\` で始まるか（区切りは `/` でもよく、大文字小文字は区別しない）。
pub fn is_named_pipe_path(value: &str) -> bool {
    let head: String = value.chars().take(PIPE_PREFIX.chars().count()).collect();
    head.replace('/', "\\").to_lowercase() == PIPE_PREFIX
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Node の `path.win32.isAbsolute`。
fn is_absolute(path: &str) -> bool {
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(first), _, _) if is_separator(first) => true,
        (Some(drive), Some(':'), Some(sep)) => drive.is_ascii_alphabetic() && is_separator(sep),
        _ => false,
    }
}

/// Node の `path.win32.join(a, b)` を、ドライブ付きの絶対パスと UNC に限って再現する
/// （区切りを `\` にそろえ、連続した区切りを 1 つにし、`.` と `..` を解決する）。
fn win32_join(base: &str, rest: &str) -> String {
    let joined = format!("{base}\\{rest}").replace('/', "\\");
    // 先頭の `\\` は UNC のルートなので残し、それ以外の連続した区切りをまとめる。
    let (root, tail) = match joined.strip_prefix("\\\\") {
        Some(unc) => ("\\\\", unc),
        None => match joined.split_once('\\') {
            Some((drive, tail)) if drive.ends_with(':') => (&joined[..=drive.len()], tail),
            _ => ("", joined.as_str()),
        },
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in tail.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("{root}{}", parts.join("\\"))
}

fn expand_home(source: &str, value: &str, home: &str) -> Result<String, PathError> {
    let expanded = if value == "~" {
        home.to_string()
    } else if value.starts_with("~/") || value.starts_with("~\\") {
        win32_join(home, &value[2..])
    } else {
        value.to_string()
    };
    if is_absolute(&expanded) {
        Ok(expanded)
    } else {
        Err(PathError::new(format!(
            "{source} must be an absolute path or start with \"~/\": {value}"
        )))
    }
}

fn require_pipe_name(source: &str, value: &str) -> Result<String, PathError> {
    if !is_named_pipe_path(value) {
        return Err(PathError::new(format!(
            "{source} must be a named pipe ({PIPE_PREFIX}<name>) on Windows: {value}"
        )));
    }
    let name: String = value.chars().skip(PIPE_PREFIX.chars().count()).collect();
    if name.is_empty() || name.chars().any(|c| c.is_control() || is_separator(c)) {
        return Err(PathError::new(format!(
            "{source} has an invalid pipe name (must be non-empty, without control characters or path separators): {value:?}"
        )));
    }
    Ok(value.to_string())
}

/// 既定のパイプ名。ユーザー名の `[A-Za-z0-9._-]` 以外は `_`（UTF-16 の 1 要素ごと）にして 32 文字までに切る。
fn default_pipe_name(dir: &str, user: &str) -> String {
    let user: String = user
        .chars()
        .flat_map(|c| {
            let keep = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
            std::iter::repeat_n(if keep { c } else { '_' }, if keep { 1 } else { c.len_utf16() })
        })
        .take(USER_MAX_LENGTH)
        .collect();
    let digest = Sha256::digest(dir.to_lowercase().as_bytes());
    let hash: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{PIPE_PREFIX}misao-{user}-{}", &hash[..HASH_LENGTH])
}

/// misao のディレクトリ（`$MISAO_DIR` > `~/.misao`）。パイプ名の元と、設定ファイル・pid・状態の置き場所になる。
pub fn windows_misao_dir(dir_env: Option<&str>, home: &str) -> Result<String, PathError> {
    match is_set(dir_env) {
        Some(dir) => expand_home("MISAO_DIR", dir, home),
        None => Ok(win32_join(
            &expand_home("home directory", home, home)?,
            DEFAULT_DIR_NAME,
        )),
    }
}

/// 接続口のパイプ名を解決する。優先順: `$MISAO_SOCKET` > `explicit` > `$MISAO_DIR` > `~/.misao`。
/// どれもパイプ名でなければならず、指定が無ければ misao のディレクトリから決まる名前になる。
pub fn resolve_pipe_name(
    env: &PathEnv,
    explicit: Option<&str>,
    home: &str,
    user: &str,
) -> Result<String, PathError> {
    let pipe = if let Some(socket) = is_set(env.socket.as_deref()) {
        require_pipe_name("MISAO_SOCKET", socket)?
    } else if let Some(path) = explicit {
        require_pipe_name("socket path", path)?
    } else {
        default_pipe_name(&windows_misao_dir(env.dir.as_deref(), home)?, user)
    };
    let length = pipe.chars().count();
    if length > MAX_PIPE_NAME_LENGTH {
        return Err(PathError::new(format!(
            "pipe name is {length} characters, exceeding the limit of {MAX_PIPE_NAME_LENGTH}: {pipe}"
        )));
    }
    Ok(pipe)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn env(socket: Option<&str>, dir: Option<&str>) -> PathEnv {
        PathEnv {
            socket: socket.map(str::to_string),
            dir: dir.map(str::to_string),
        }
    }

    fn resolve(e: &PathEnv, explicit: Option<&str>, home: &str, user: &str) -> String {
        resolve_pipe_name(e, explicit, home, user).unwrap()
    }

    /// 期待値は SDK の `resolveSocketPath({ platform: 'win32' })` の出力（Node 24 で生成）。
    #[test]
    fn default_names_match_the_sdk() {
        let home = "C:\\Users\\Test";
        assert_eq!(
            resolve(&env(None, None), None, home, "Test"),
            "\\\\.\\pipe\\misao-Test-eca123378d17f0ff"
        );
        assert_eq!(
            resolve(&env(None, None), None, "C:\\Users\\Test\\", "a😀b"),
            "\\\\.\\pipe\\misao-a__b-eca123378d17f0ff"
        );
        assert_eq!(
            resolve(&env(None, Some("~/m")), None, home, "Test"),
            "\\\\.\\pipe\\misao-Test-2776d868a9f0e9c3"
        );
        // 絶対パスの $MISAO_DIR は、正規化せずそのまま（小文字にして）ハッシュする
        assert_eq!(
            resolve(&env(None, Some("D:\\Work\\Misao")), None, home, "Test"),
            "\\\\.\\pipe\\misao-Test-25c9fd6fe1e45986"
        );
        assert_eq!(
            resolve(
                &env(None, Some("~\\a\\..\\b")),
                None,
                "C:\\Users\\Test User",
                "Test User/ねこ"
            ),
            "\\\\.\\pipe\\misao-Test_User___-8ebf97f571c05d9e"
        );
        assert_eq!(
            resolve(
                &env(None, Some("d:/x//y/./z")),
                None,
                "C:/Users/T",
                "averyveryverylongusername_with_more_than_32_chars"
            ),
            "\\\\.\\pipe\\misao-averyveryverylongusername_with_m-3b6fa2c1b3f60de3"
        );
    }

    #[test]
    fn explicit_names_win_in_priority_order_and_must_be_pipes() {
        let home = "C:\\Users\\Test";
        let all = env(Some("\\\\.\\pipe\\from-env"), Some("D:\\d"));
        assert_eq!(
            resolve(&all, Some("\\\\.\\pipe\\x"), home, "u"),
            "\\\\.\\pipe\\from-env"
        );
        assert_eq!(
            resolve(&env(None, Some("D:\\d")), Some("//./pipe/other"), home, "u"),
            "//./pipe/other"
        );
        let err = resolve_pipe_name(&env(Some("C:\\x.sock"), None), None, home, "u").unwrap_err();
        assert_eq!(
            err.to_string(),
            "MISAO_SOCKET must be a named pipe (\\\\.\\pipe\\<name>) on Windows: C:\\x.sock"
        );
        for bad in [
            "\\\\.\\pipe\\",
            "\\\\.\\pipe\\a\\b",
            "\\\\.\\pipe\\a/b",
            "\\\\.\\pipe\\a\u{1}b",
        ] {
            let err = resolve_pipe_name(&env(Some(bad), None), None, home, "u").unwrap_err();
            assert!(err.to_string().contains("invalid pipe name"), "{bad:?}: {err}");
        }
        let long = format!("{PIPE_PREFIX}{}", "a".repeat(MAX_PIPE_NAME_LENGTH));
        let err = resolve_pipe_name(&env(Some(&long), None), None, home, "u").unwrap_err();
        assert!(
            err.to_string().starts_with("pipe name is 265 characters"),
            "{err}"
        );
    }

    #[test]
    fn a_relative_misao_dir_is_rejected() {
        let err = resolve_pipe_name(&env(None, Some("rel")), None, "C:\\Users\\Test", "u").unwrap_err();
        assert_eq!(
            err.to_string(),
            "MISAO_DIR must be an absolute path or start with \"~/\": rel"
        );
    }

    #[test]
    fn pipe_path_detection_ignores_separators_and_case() {
        assert!(is_named_pipe_path("\\\\.\\PIPE\\x"));
        assert!(is_named_pipe_path("//./pipe/x"));
        assert!(!is_named_pipe_path("C:\\pipe\\x"));
        assert!(!is_named_pipe_path("/tmp/misao.sock"));
    }
}
