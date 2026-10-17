//! ソケットパスの解決。`packages/sdk-node/src/socket-path.ts` と同じ規則。

use std::path::{Component, Path, PathBuf};

/// `sockaddr_un.sun_path` は Linux で 108 バイト（終端 NUL を含む）。
#[cfg(not(windows))]
pub const MAX_SOCKET_PATH_BYTES: usize = 107;

#[cfg(not(windows))]
const SOCKET_FILE_NAME: &str = "misao.sock";
const CONFIG_FILE_NAME: &str = "misao.json";
const DEFAULT_DIR_NAME: &str = ".misao";

/// misao のパス設定（`$MISAO_SOCKET` / `$MISAO_DIR` / 明示指定）が不正。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct PathError(String);

impl PathError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// 解決に使う環境変数。空文字は未設定として扱う。
#[derive(Debug, Default, Clone)]
pub struct PathEnv {
    pub socket: Option<String>,
    pub dir: Option<String>,
}

fn set(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

/// 字句的な正規化（Node の `path.resolve` / `path.join` と同じ: `.` を捨て、`..` で 1 つ戻る）。
/// シンボリックリンクは解決しない。ルートより上への `..` は捨てる。
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn expand_home(source: &str, value: &str, home: &str) -> Result<PathBuf, PathError> {
    let expanded = if value == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\").filter(|_| cfg!(windows)))
    {
        normalize_lexical(&Path::new(home).join(rest))
    } else {
        PathBuf::from(value)
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Err(PathError(format!(
            "{source} must be an absolute path or start with \"~/\": {value}"
        )))
    }
}

/// 設定ファイルの出所を伝えるための、ディレクトリ候補の指定元。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirOrigin {
    MisaoDir,
    Default,
}

/// ディレクトリ候補を優先順に返す: `$MISAO_DIR`（設定時のみ）> `~/.misao`。
pub fn resolve_misao_dirs(dir_env: Option<&str>, home: &str) -> Result<Vec<(PathBuf, DirOrigin)>, PathError> {
    let fallback = (
        expand_home("home directory", home, home)?.join(DEFAULT_DIR_NAME),
        DirOrigin::Default,
    );
    match set(dir_env) {
        None => Ok(vec![fallback]),
        Some(dir) => Ok(vec![
            (expand_home("MISAO_DIR", dir, home)?, DirOrigin::MisaoDir),
            fallback,
        ]),
    }
}

/// `$MISAO_DIR` > `~/.misao` の順で、設定ファイル `misao.json` の候補を返す。
pub fn config_candidates_in_dirs(
    dir_env: Option<&str>,
    home: &str,
) -> Result<Vec<(PathBuf, DirOrigin)>, PathError> {
    Ok(resolve_misao_dirs(dir_env, home)?
        .into_iter()
        .map(|(dir, origin)| (normalize_lexical(&dir.join(CONFIG_FILE_NAME)), origin))
        .collect())
}

/// ソケットパスを解決する。
/// 優先順: `$MISAO_SOCKET` > `explicit` > `$MISAO_DIR/misao.sock` > `~/.misao/misao.sock`。
/// Windows の接続口は名前付きパイプ（Unix ソケットのパスではない）で、名前は `pipe_name` が決める。
#[cfg(windows)]
pub fn resolve_socket_path(env: &PathEnv, explicit: Option<&str>, home: &str) -> Result<PathBuf, PathError> {
    let user = crate::win_user::current_user_name()
        .map_err(|e| PathError::new(format!("cannot determine the user name: {e}")))?;
    crate::pipe_name::resolve_pipe_name(env, explicit, home, &user).map(PathBuf::from)
}

#[cfg(not(windows))]
pub fn resolve_socket_path(env: &PathEnv, explicit: Option<&str>, home: &str) -> Result<PathBuf, PathError> {
    let resolved = if let Some(socket) = set(env.socket.as_deref()) {
        expand_home("MISAO_SOCKET", socket, home)?
    } else if let Some(path) = explicit {
        expand_home("socket path", path, home)?
    } else {
        let dirs = resolve_misao_dirs(env.dir.as_deref(), home)?;
        normalize_lexical(&dirs[0].0.join(SOCKET_FILE_NAME))
    };
    let bytes = resolved.as_os_str().len();
    if bytes > MAX_SOCKET_PATH_BYTES {
        return Err(PathError(format!(
            "socket path is {bytes} bytes, exceeding the limit of {MAX_SOCKET_PATH_BYTES}: {}",
            resolved.display()
        )));
    }
    Ok(resolved)
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn env(socket: Option<&str>, dir: Option<&str>) -> PathEnv {
        PathEnv {
            socket: socket.map(str::to_string),
            dir: dir.map(str::to_string),
        }
    }

    fn resolve(e: &PathEnv, explicit: Option<&str>) -> Result<PathBuf, PathError> {
        resolve_socket_path(e, explicit, "/home/u")
    }

    #[test]
    fn defaults_to_home_dot_misao() {
        assert_eq!(
            resolve(&env(None, None), None).unwrap(),
            PathBuf::from("/home/u/.misao/misao.sock")
        );
    }

    #[test]
    fn priority_is_env_socket_then_explicit_then_dir() {
        let all = env(Some("/s/env.sock"), Some("/d"));
        assert_eq!(
            resolve(&all, Some("/s/x.sock")).unwrap(),
            PathBuf::from("/s/env.sock")
        );
        let no_sock = env(None, Some("/d"));
        assert_eq!(
            resolve(&no_sock, Some("/s/x.sock")).unwrap(),
            PathBuf::from("/s/x.sock")
        );
        assert_eq!(resolve(&no_sock, None).unwrap(), PathBuf::from("/d/misao.sock"));
    }

    #[test]
    fn empty_env_values_count_as_unset() {
        assert_eq!(
            resolve(&env(Some(""), Some("")), None).unwrap(),
            PathBuf::from("/home/u/.misao/misao.sock")
        );
    }

    #[test]
    fn expands_tilde_and_rejects_relative() {
        assert_eq!(
            resolve(&env(None, Some("~/m")), None).unwrap(),
            PathBuf::from("/home/u/m/misao.sock")
        );
        assert_eq!(
            resolve(&env(None, None), Some("~/a.sock")).unwrap(),
            PathBuf::from("/home/u/a.sock")
        );
        let err = resolve(&env(Some("rel.sock"), None), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "MISAO_SOCKET must be an absolute path or start with \"~/\": rel.sock"
        );
    }

    #[test]
    fn normalizes_dot_segments_like_node_path_join() {
        assert_eq!(
            resolve(&env(None, Some("/d/./x/../y")), None).unwrap(),
            PathBuf::from("/d/y/misao.sock")
        );
        assert_eq!(
            resolve(&env(None, Some("~/a/../b")), None).unwrap(),
            PathBuf::from("/home/u/b/misao.sock")
        );
        assert_eq!(normalize_lexical(Path::new("/../a//b/.")), PathBuf::from("/a/b"));
        assert_eq!(normalize_lexical(Path::new("a/../../b")), PathBuf::from("../b"));
    }

    #[test]
    fn rejects_paths_over_107_bytes() {
        let long = format!("/{}", "a".repeat(107));
        let err = resolve(&env(Some(&long), None), None).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("socket path is 108 bytes, exceeding the limit of 107: "),
            "{err}"
        );
        let ok = format!("/{}", "a".repeat(106));
        assert!(resolve(&env(Some(&ok), None), None).is_ok());
    }
}
