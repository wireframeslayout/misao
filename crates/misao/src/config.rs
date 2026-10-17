//! 設定ファイル `misao.json` の探索と検証。`socket` / `logLevel` / `scrollback` / `rings` をデーモンが使う。
//! `keys` は attach の割り当て（prefix とアクションキー）で、CLI の attach が使う。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use misao_daemon::{Limits, LogLevel};
use serde_json::{Map, Value};

use crate::keys::{ACTIONS, KeyBindings, parse_key_spec, parse_prefix_key_spec};
use crate::socket_path::{DirOrigin, PathEnv, config_candidates_in_dirs, resolve_socket_path};

const KNOWN_KEYS: [&str; 5] = ["keys", "scrollback", "rings", "socket", "logLevel"];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Path(#[from] crate::socket_path::PathError),
}

fn fail<T>(message: String) -> Result<T, ConfigError> {
    Err(ConfigError::Message(message))
}

#[derive(Debug, Clone)]
pub struct Config {
    /// `misao.json` の `socket`（未解決。`~/` 展開と長さの検査は呼び出し側で行う）。
    pub socket: Option<String>,
    /// デーモンのログ閾値。`serve`（unix のみ）が使う。
    #[cfg_attr(not(unix), allow(dead_code))]
    pub log_level: LogLevel,
    /// スクロールバックとリングの上限。`serve`（unix のみ）が使う。
    #[cfg_attr(not(unix), allow(dead_code))]
    pub limits: Limits,
    /// attach のキー割り当て。
    pub keys: KeyBindings,
}

#[derive(Debug)]
pub struct LoadedConfig {
    /// 読み込んだ設定ファイル。見つからず既定値で動く場合は None。
    pub source: Option<PathBuf>,
    pub config: Config,
    /// 利用者に見せる警告（未知のキーなど）。
    pub warnings: Vec<String>,
}

pub struct LoadInput<'a> {
    /// `--config PATH`
    pub flag_path: Option<&'a Path>,
    /// `$MISAO_CONFIG`（空文字は未設定扱い）
    pub env_config: Option<&'a str>,
    pub env: &'a PathEnv,
    pub home: &'a str,
}

struct Candidate {
    path: PathBuf,
    /// 明示指定（`--config` / `$MISAO_CONFIG`）。無ければエラーにする。
    explicit: bool,
    origin: String,
}

fn candidates(input: &LoadInput<'_>) -> Result<Vec<Candidate>, ConfigError> {
    let mut out = Vec::new();
    if let Some(p) = input.flag_path {
        out.push(Candidate {
            path: p.to_path_buf(),
            explicit: true,
            origin: "--config".into(),
        });
    }
    if let Some(p) = input.env_config.filter(|v| !v.is_empty()) {
        out.push(Candidate {
            path: PathBuf::from(p),
            explicit: true,
            origin: "$MISAO_CONFIG".into(),
        });
    }
    let dir_env = input.env.dir.as_deref();
    for (path, origin) in config_candidates_in_dirs(dir_env, input.home)? {
        let origin = match origin {
            DirOrigin::MisaoDir => "$MISAO_DIR",
            DirOrigin::Default => "default",
        };
        out.push(Candidate {
            path,
            explicit: false,
            origin: origin.into(),
        });
    }
    Ok(out)
}

/// 設定ファイルを探索・検証する。最初に見つかった 1 ファイルだけを使う（マージしない）。
/// 優先順: `--config` > `$MISAO_CONFIG` > `$MISAO_DIR/misao.json` > `~/.misao/misao.json`。
pub fn load_config(input: &LoadInput<'_>) -> Result<LoadedConfig, ConfigError> {
    if input.flag_path.is_some_and(|p| p.as_os_str().is_empty()) {
        return fail("--config requires a non-empty path".into());
    }
    for c in candidates(input)? {
        match fs::read_to_string(&c.path) {
            Ok(text) => return parse_config(&c.path, &text),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !c.explicit => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return fail(format!(
                    "config file from {} not found: {}",
                    c.origin,
                    c.path.display()
                ));
            }
            Err(e) => {
                return fail(format!(
                    "cannot read config file from {} ({}): {e}",
                    c.origin,
                    c.path.display()
                ));
            }
        }
    }
    Ok(LoadedConfig {
        source: None,
        config: Config {
            socket: None,
            log_level: LogLevel::DEFAULT,
            limits: Limits::default(),
            keys: KeyBindings::default(),
        },
        warnings: Vec::new(),
    })
}

/// ソケットパスを解決する（`$MISAO_SOCKET` > 設定の `socket` > `$MISAO_DIR` > `~/.misao`）。
/// 不正なら、原因（環境変数名など）と読み込んだ設定ファイルを添えた設定エラー。
pub fn resolve_socket(loaded: &LoadedConfig, env: &PathEnv, home: &str) -> Result<PathBuf, ConfigError> {
    resolve_socket_path(env, loaded.config.socket.as_deref(), home).map_err(|e| {
        let context = loaded
            .source
            .as_ref()
            .map_or(String::new(), |s| format!(" (config: {})", s.display()));
        ConfigError::Message(format!("invalid path setting: {e}{context}"))
    })
}

fn parse_config(path: &Path, text: &str) -> Result<LoadedConfig, ConfigError> {
    let source = path.display();
    let value: Value = serde_json::from_str(text)
        .map_err(|e| ConfigError::Message(format!("invalid JSON in {source}: {e}")))?;
    let Value::Object(obj) = value else {
        return fail(format!("invalid config {source}:\n  (root): expected object"));
    };
    let issues = validate(&obj);
    if !issues.is_empty() {
        return fail(format!("invalid config {source}:\n{}", issues.join("\n")));
    }
    let warnings = unknown_keys(&obj)
        .iter()
        .map(|k| format!("{source}: unknown key \"{k}\" ignored"))
        .collect();
    let socket = obj.get("socket").and_then(Value::as_str).map(str::to_string);
    let log_level = obj
        .get("logLevel")
        .and_then(Value::as_str)
        .and_then(LogLevel::parse)
        .unwrap_or(LogLevel::DEFAULT);
    Ok(LoadedConfig {
        source: Some(path.to_path_buf()),
        config: Config {
            socket,
            log_level,
            limits: read_limits(&obj),
            keys: read_keys(&obj),
        },
        warnings,
    })
}

/// 検証済みの設定（`validate` を通っている）から上限を読む。値が無い項目は既定。
fn read_limits(obj: &Map<String, Value>) -> Limits {
    let defaults = Limits::default();
    let ring = |key: &str, default: usize| {
        obj.get("rings")
            .and_then(|r| r.get(key))
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(default)
    };
    Limits {
        scrollback: obj
            .get("scrollback")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(defaults.scrollback),
        raw_ring_bytes: ring("rawBytes", defaults.raw_ring_bytes),
        lines_ring_bytes: ring("linesBytes", defaults.lines_ring_bytes),
        events: ring("events", defaults.events),
    }
}

/// 項目名・違反時のメッセージ・値が正しいかの判定。
type Check = (&'static str, &'static str, fn(&Value) -> bool);

fn validate(obj: &Map<String, Value>) -> Vec<String> {
    let checks: [Check; 5] = [
        ("socket", "expected a non-empty string", |v| {
            v.as_str().is_some_and(|s| !s.is_empty())
        }),
        (
            "logLevel",
            "expected one of \"error\", \"warn\", \"info\", \"debug\"",
            |v| v.as_str().and_then(LogLevel::parse).is_some(),
        ),
        ("scrollback", "expected an integer >= 1", |v| {
            v.as_u64().is_some_and(|n| n >= 1)
        }),
        ("keys", "expected an object", Value::is_object),
        ("rings", "expected an object", Value::is_object),
    ];
    let mut issues: Vec<String> = checks
        .iter()
        .filter(|(key, _, valid)| obj.get(*key).is_some_and(|v| !valid(v)))
        .map(|(key, message, _)| format!("  {key}: {message}"))
        .collect();
    if let Some(Value::Object(rings)) = obj.get("rings") {
        issues.extend(validate_rings(rings));
    }
    if let Some(Value::Object(keys)) = obj.get("keys") {
        issues.extend(validate_keys(keys).0);
    }
    issues
}

const RING_KEYS: [&str; 3] = ["rawBytes", "linesBytes", "events"];
const KEY_NAMES: [&str; 5] = ["prefix", "detach", "next", "prev", "list"];

/// 未知のキー（`rings.x` のように入れ子の経路で返す）。Node 版は取り除いて警告にする。
fn unknown_keys(obj: &Map<String, Value>) -> Vec<String> {
    let mut out: Vec<String> = obj
        .keys()
        .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    for (section, known) in [("rings", &RING_KEYS[..]), ("keys", &KEY_NAMES[..])] {
        if let Some(Value::Object(inner)) = obj.get(section) {
            out.extend(
                inner
                    .keys()
                    .filter(|k| !known.contains(&k.as_str()))
                    .map(|k| format!("{section}.{k}")),
            );
        }
    }
    out
}

/// `rings` の値は 1 以上の整数。
fn validate_rings(rings: &Map<String, Value>) -> Vec<String> {
    rings
        .iter()
        .filter(|(key, value)| RING_KEYS.contains(&key.as_str()) && !value.as_u64().is_some_and(|n| n >= 1))
        .map(|(key, _)| format!("  rings.{key}: expected an integer >= 1"))
        .collect()
}

/// `keys` の各キーの指定と、キー同士の重複・prefix との衝突。個別の指定が不正なら重複の検査はしない（zod と同じ）。
fn validate_keys(keys: &Map<String, Value>) -> (Vec<String>, Option<KeyBindings>) {
    let mut issues = Vec::new();
    let mut parsed = KeyBindings::default();
    for name in KEY_NAMES {
        let Some(value) = keys.get(name) else { continue };
        let result = match value.as_str() {
            None => Err("expected string".to_string()),
            Some(spec) if name == "prefix" => parse_prefix_key_spec(spec),
            Some(spec) => parse_key_spec(spec),
        };
        match result {
            Ok(code) => set_key(&mut parsed, name, code),
            Err(message) => issues.push(format!("  keys.{name}: {message}")),
        }
    }
    if !issues.is_empty() {
        return (issues, None);
    }
    let mut seen: Vec<(u8, &str)> = Vec::new();
    for action in ACTIONS {
        let code = parsed.action_key(action).unwrap_or_default();
        if let Some((_, other)) = seen.iter().find(|(c, _)| *c == code) {
            issues.push(format!("  keys.{action}: keys.{action} duplicates keys.{other}"));
        }
        seen.push((code, action));
        if code == parsed.prefix {
            issues.push(format!(
                "  keys.{action}: keys.{action} collides with keys.prefix"
            ));
        }
    }
    (issues, Some(parsed))
}

fn set_key(keys: &mut KeyBindings, name: &str, code: u8) {
    match name {
        "prefix" => keys.prefix = code,
        "detach" => keys.detach = code,
        "next" => keys.next = code,
        "prev" => keys.prev = code,
        _ => keys.list = code,
    }
}

/// 検証済みの設定から keys を読む。
fn read_keys(obj: &Map<String, Value>) -> KeyBindings {
    obj.get("keys")
        .and_then(Value::as_object)
        .and_then(|keys| validate_keys(keys).1)
        .unwrap_or_default()
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn load(dir: &TempDir, flag: Option<&str>, json: Option<&str>) -> Result<LoadedConfig, ConfigError> {
        let file = dir.path().join("misao.json");
        if let Some(j) = json {
            fs::write(&file, j).unwrap();
        }
        let env = PathEnv {
            socket: None,
            dir: Some(dir.path().to_string_lossy().into_owned()),
        };
        let flag_path = flag.map(|f| dir.path().join(f));
        load_config(&LoadInput {
            flag_path: flag_path.as_deref(),
            env_config: None,
            env: &env,
            home: "/nonexistent",
        })
    }

    #[test]
    fn reads_socket_and_log_level_from_misao_dir() {
        let dir = TempDir::new().unwrap();
        let loaded = load(&dir, None, Some(r#"{"logLevel":"warn","socket":"/s.sock"}"#)).unwrap();
        assert_eq!(loaded.config.log_level, LogLevel::Warn);
        assert_eq!(loaded.config.socket.as_deref(), Some("/s.sock"));
    }

    #[test]
    fn missing_implicit_file_means_defaults() {
        let dir = TempDir::new().unwrap();
        let loaded = load(&dir, None, None).unwrap();
        assert_eq!(loaded.config.log_level, LogLevel::Info);
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let dir = TempDir::new().unwrap();
        let err = load(&dir, Some("typo.json"), None).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("config file from --config not found: "),
            "{err}"
        );
    }

    #[test]
    fn limits_default_to_the_node_daemon_values() {
        let dir = TempDir::new().unwrap();
        let limits = load(&dir, None, Some("{}")).unwrap().config.limits;
        assert_eq!(limits, Limits::default());
        assert_eq!(
            (
                limits.scrollback,
                limits.raw_ring_bytes,
                limits.lines_ring_bytes,
                limits.events
            ),
            (5000, 1024 * 1024, 4 * 1024 * 1024, 1000)
        );
    }

    #[test]
    fn limits_are_read_from_scrollback_and_rings() {
        let dir = TempDir::new().unwrap();
        let json = r#"{"scrollback":10,"rings":{"rawBytes":20,"linesBytes":30,"events":40}}"#;
        let limits = load(&dir, None, Some(json)).unwrap().config.limits;
        assert_eq!(
            (
                limits.scrollback,
                limits.raw_ring_bytes,
                limits.lines_ring_bytes,
                limits.events
            ),
            (10, 20, 30, 40)
        );
        let partial = load(&dir, None, Some(r#"{"rings":{"linesBytes":5}}"#))
            .unwrap()
            .config
            .limits;
        assert_eq!(partial.lines_ring_bytes, 5);
        assert_eq!(partial.raw_ring_bytes, Limits::default().raw_ring_bytes);
    }

    #[test]
    fn invalid_rings_are_rejected() {
        let dir = TempDir::new().unwrap();
        let err = load(&dir, None, Some(r#"{"rings":{"rawBytes":0}}"#))
            .unwrap_err()
            .to_string();
        assert!(err.contains("rings.rawBytes: expected an integer >= 1"), "{err}");
        // 未知のキーは Node 版と同じく取り除いて警告にする
        let warned = load(&dir, None, Some(r#"{"rings":{"bogus":1},"keys":{"x":"d"}}"#)).unwrap();
        assert_eq!(warned.warnings.len(), 2, "{:?}", warned.warnings);
        assert!(warned.warnings[0].ends_with("unknown key \"rings.bogus\" ignored"));
        assert!(warned.warnings[1].ends_with("unknown key \"keys.x\" ignored"));
    }

    #[test]
    fn keys_are_read_and_validated_like_the_node_schema() {
        let dir = TempDir::new().unwrap();
        let keys = load(&dir, None, Some(r#"{"keys":{"prefix":"C-a","detach":"q"}}"#))
            .unwrap()
            .config
            .keys;
        assert_eq!((keys.prefix, keys.detach, keys.next), (1, b'q', b'n'));
        let err = |json: &str| load(&dir, None, Some(json)).unwrap_err().to_string();
        assert!(err(r#"{"keys":{"prefix":"d"}}"#).contains("keys.prefix: invalid prefix key"));
        assert!(err(r#"{"keys":{"detach":"n"}}"#).contains("keys.next: keys.next duplicates keys.detach"));
        assert!(
            err(r#"{"keys":{"prefix":"C-d","detach":"C-d"}}"#)
                .contains("keys.detach: keys.detach collides with keys.prefix")
        );
        assert!(err(r#"{"keys":{"next":5}}"#).contains("keys.next: expected string"));
    }

    #[test]
    fn unknown_keys_warn_and_bad_values_fail() {
        let dir = TempDir::new().unwrap();
        let loaded = load(&dir, None, Some(r#"{"extra":1}"#)).unwrap();
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].ends_with("unknown key \"extra\" ignored"));
        let err = load(&dir, None, Some(r#"{"logLevel":"loud"}"#)).unwrap_err();
        assert!(err.to_string().contains("logLevel: expected one of"), "{err}");
    }

    // ---- Node 版 `config/load.test.ts` の観点 ----

    struct Setup<'a> {
        flag: Option<&'a Path>,
        env_config: Option<&'a str>,
        dir: Option<&'a str>,
        socket: Option<&'a str>,
        home: &'a Path,
    }

    fn load_with(setup: &Setup<'_>) -> Result<LoadedConfig, ConfigError> {
        let env = PathEnv {
            socket: setup.socket.map(str::to_string),
            dir: setup.dir.map(str::to_string),
        };
        load_config(&LoadInput {
            flag_path: setup.flag,
            env_config: setup.env_config,
            env: &env,
            home: setup.home.to_str().unwrap(),
        })
    }

    fn setup(home: &Path) -> Setup<'_> {
        Setup {
            flag: None,
            env_config: None,
            dir: None,
            socket: None,
            home,
        }
    }

    fn write(dir: &Path, name: &str, json: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        fs::write(&file, json).unwrap();
        file
    }

    #[test]
    fn no_file_means_defaults_with_the_default_socket() {
        let home = TempDir::new().unwrap();
        let loaded = load_with(&setup(home.path())).unwrap();
        assert!(loaded.source.is_none() && loaded.warnings.is_empty());
        assert_eq!(loaded.config.keys, KeyBindings::default());
        assert_eq!(loaded.config.limits, Limits::default());
        let socket = resolve_socket(&loaded, &PathEnv::default(), home.path().to_str().unwrap()).unwrap();
        assert_eq!(socket, home.path().join(".misao/misao.sock"));
    }

    #[test]
    fn search_order_is_flag_then_misao_config_then_misao_dir_then_home() {
        let root = TempDir::new().unwrap();
        let flag = write(root.path(), "f.json", r#"{"scrollback":1}"#);
        let envfile = write(root.path(), "e.json", r#"{"scrollback":2}"#);
        let dir = root.path().join("mdir");
        write(&dir, "misao.json", r#"{"scrollback":3}"#);
        let home = root.path().join("home");
        write(&home.join(".misao"), "misao.json", r#"{"scrollback":4}"#);
        let pick = |flag: Option<&Path>, env_config: Option<&str>, dir: Option<&str>| {
            let s = Setup {
                flag,
                env_config,
                dir,
                socket: None,
                home: &home,
            };
            load_with(&s).unwrap().config.limits.scrollback
        };
        let (e, d) = (envfile.to_str().unwrap(), dir.to_str().unwrap());
        assert_eq!(pick(Some(&flag), Some(e), Some(d)), 1);
        assert_eq!(pick(None, Some(e), Some(d)), 2);
        assert_eq!(pick(None, None, Some(d)), 3);
        assert_eq!(pick(None, None, None), 4);
        assert_eq!(pick(None, Some(""), Some("")), 4, "empty variables are unset");
    }

    #[test]
    fn a_missing_misao_dir_falls_through_and_an_unreadable_one_fails() {
        let root = TempDir::new().unwrap();
        let missing = root.path().join("nonexistent");
        let s = Setup {
            dir: missing.to_str(),
            ..setup(root.path())
        };
        assert!(load_with(&s).unwrap().source.is_none());
        let eisdir = root.path().join("eisdir");
        fs::create_dir_all(eisdir.join("misao.json")).unwrap();
        let s = Setup {
            dir: eisdir.to_str(),
            ..setup(root.path())
        };
        assert!(load_with(&s).unwrap_err().to_string().contains("from $MISAO_DIR"));
    }

    #[test]
    fn a_missing_explicit_file_names_its_origin() {
        let root = TempDir::new().unwrap();
        let missing = root.path().join("missing.json");
        let s = Setup {
            env_config: missing.to_str(),
            ..setup(root.path())
        };
        assert_eq!(
            load_with(&s).unwrap_err().to_string(),
            format!("config file from $MISAO_CONFIG not found: {}", missing.display())
        );
        let s = Setup {
            flag: Some(Path::new("")),
            ..setup(root.path())
        };
        assert!(
            load_with(&s)
                .unwrap_err()
                .to_string()
                .contains("--config requires a non-empty path")
        );
    }

    #[test]
    fn bad_json_wrong_types_and_mixed_problems_fail() {
        let root = TempDir::new().unwrap();
        let try_json = |json: &str| {
            let file = write(root.path(), "bad.json", json);
            load_with(&Setup {
                flag: Some(&file),
                ..setup(root.path())
            })
        };
        assert!(
            try_json("{ nope")
                .unwrap_err()
                .to_string()
                .contains("invalid JSON")
        );
        for bad in [
            r#"{"scrollback":"many"}"#,
            r#"{"scrollback":0}"#,
            r#"{"logLevel":"trace"}"#,
            r#"{"keys":{"detach":"xx"}}"#,
            "[]",
            r#"{"extra":1,"scrollback":"x"}"#,
        ] {
            assert!(try_json(bad).is_err(), "{bad}");
        }
        let loaded =
            try_json(r#"{"scrollback":10,"extra":1,"rings":{"events":5,"bogus":true},"keys":{"zzz":"q"}}"#)
                .unwrap();
        assert_eq!(
            (loaded.config.limits.scrollback, loaded.config.limits.events),
            (10, 5)
        );
        assert_eq!(loaded.warnings.len(), 3);
        for key in ["extra", "rings.bogus", "keys.zzz"] {
            assert!(
                loaded.warnings.iter().any(|w| w.contains(&format!("\"{key}\""))),
                "{key}"
            );
        }
    }

    #[test]
    fn the_socket_setting_loses_to_the_environment_and_bad_paths_name_their_cause() {
        let root = TempDir::new().unwrap();
        let file = write(root.path(), "s.json", r#"{"socket":"/tmp/from-config.sock"}"#);
        let home = root.path().to_str().unwrap();
        let resolve = |socket: Option<&str>| {
            let s = Setup {
                flag: Some(&file),
                socket,
                ..setup(root.path())
            };
            let loaded = load_with(&s).unwrap();
            let env = PathEnv {
                socket: socket.map(str::to_string),
                dir: None,
            };
            resolve_socket(&loaded, &env, home)
        };
        assert_eq!(resolve(None).unwrap(), PathBuf::from("/tmp/from-config.sock"));
        assert_eq!(
            resolve(Some("/tmp/from-env.sock")).unwrap(),
            PathBuf::from("/tmp/from-env.sock")
        );
        assert!(
            resolve(Some("rel.sock"))
                .unwrap_err()
                .to_string()
                .contains("MISAO_SOCKET")
        );

        let long = write(
            root.path(),
            "long.json",
            &format!(r#"{{"socket":"/{}.sock"}}"#, "a".repeat(120)),
        );
        let loaded = load_with(&Setup {
            flag: Some(&long),
            ..setup(root.path())
        })
        .unwrap();
        let message = resolve_socket(&loaded, &PathEnv::default(), home)
            .unwrap_err()
            .to_string();
        assert!(message.contains(long.to_str().unwrap()), "{message}");
    }

    #[test]
    fn misao_dir_expands_a_tilde_and_rejects_relative_paths() {
        let root = TempDir::new().unwrap();
        write(&root.path().join("d"), "misao.json", r#"{"scrollback":7}"#);
        let s = Setup {
            dir: Some("~/d"),
            ..setup(root.path())
        };
        let loaded = load_with(&s).unwrap();
        assert_eq!(loaded.source, Some(root.path().join("d/misao.json")));
        assert_eq!(loaded.config.limits.scrollback, 7);
        let s = Setup {
            dir: Some("rel"),
            ..setup(root.path())
        };
        assert!(load_with(&s).unwrap_err().to_string().contains("MISAO_DIR"));
    }

    #[test]
    fn the_detach_key_and_prefix_can_be_changed() {
        let root = TempDir::new().unwrap();
        let file = write(root.path(), "k.json", r#"{"keys":{"detach":"x","prefix":"C-a"}}"#);
        let keys = load_with(&Setup {
            flag: Some(&file),
            ..setup(root.path())
        })
        .unwrap()
        .config
        .keys;
        assert_eq!((keys.detach, keys.prefix, keys.next), (0x78, 0x01, 0x6e));
        let bad = write(root.path(), "p.json", r#"{"keys":{"prefix":"a"}}"#);
        let error = load_with(&Setup {
            flag: Some(&bad),
            ..setup(root.path())
        })
        .unwrap_err();
        assert!(error.to_string().contains("prefix must be a control key"));
    }
}
