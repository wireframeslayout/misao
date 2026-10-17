use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

/// 子プロセスの環境: デーモンの環境から mux 系の変数を除いて引き継ぎ、misao 用を足す。overrides が最後に上書きする。
/// Node 版の `buildChildEnv`。
pub fn build_child_env(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    pane_id: &str,
    socket_path: &Path,
    overrides: &BTreeMap<String, String>,
) -> Vec<(OsString, OsString)> {
    let mut env: BTreeMap<OsString, OsString> = base
        .into_iter()
        .filter(|(key, _)| !is_mux_variable(&key.to_string_lossy()))
        .collect();
    let mut set = |key: &str, value: &std::ffi::OsStr| {
        env.insert(OsString::from(key), value.to_os_string());
    };
    set("MISAO_SOCKET", socket_path.as_os_str());
    set("MISAO_PANE_ID", pane_id.as_ref());
    set("TERM", "xterm-256color".as_ref());
    set("COLORTERM", "truecolor".as_ref());
    for (key, value) in overrides {
        set(key, value.as_ref());
    }
    env.into_iter().collect()
}

/// ペインの中で別の端末多重化の環境を引き継がないための除外。
fn is_mux_variable(name: &str) -> bool {
    matches!(name, "TMUX" | "TMUX_PANE" | "STY") || name.starts_with("ZELLIJ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        items.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    fn get(env: &[(OsString, OsString)], key: &str) -> Option<String> {
        env.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.to_string_lossy().into_owned())
    }

    #[test]
    fn inherits_the_base_env_except_mux_variables() {
        let base = pairs(&[
            ("PATH", "/bin"),
            ("HOME", "/h"),
            ("TMUX", "x"),
            ("TMUX_PANE", "%1"),
            ("STY", "s"),
            ("ZELLIJ", "0"),
            ("ZELLIJ_SESSION_NAME", "z"),
        ]);
        let env = build_child_env(base, "p_1", Path::new("/s.sock"), &BTreeMap::new());
        assert_eq!(get(&env, "PATH").as_deref(), Some("/bin"));
        assert_eq!(get(&env, "HOME").as_deref(), Some("/h"));
        for gone in ["TMUX", "TMUX_PANE", "STY", "ZELLIJ", "ZELLIJ_SESSION_NAME"] {
            assert_eq!(get(&env, gone), None, "{gone}");
        }
    }

    #[test]
    fn adds_misao_variables_and_terminal_type() {
        let env = build_child_env([], "p_9", Path::new("/run/misao.sock"), &BTreeMap::new());
        assert_eq!(get(&env, "MISAO_SOCKET").as_deref(), Some("/run/misao.sock"));
        assert_eq!(get(&env, "MISAO_PANE_ID").as_deref(), Some("p_9"));
        assert_eq!(get(&env, "TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(get(&env, "COLORTERM").as_deref(), Some("truecolor"));
    }

    #[test]
    fn overrides_win_over_everything_else() {
        let base = pairs(&[("TERM", "dumb"), ("A", "base")]);
        let overrides = BTreeMap::from([
            ("A".to_string(), "over".to_string()),
            ("TERM".to_string(), "vt100".to_string()),
            ("MISAO_PANE_ID".to_string(), "custom".to_string()),
        ]);
        let env = build_child_env(base, "p_1", Path::new("/s"), &overrides);
        assert_eq!(get(&env, "A").as_deref(), Some("over"));
        assert_eq!(get(&env, "TERM").as_deref(), Some("vt100"));
        assert_eq!(get(&env, "MISAO_PANE_ID").as_deref(), Some("custom"));
    }
}
