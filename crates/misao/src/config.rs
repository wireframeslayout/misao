//! 設定ファイル `misao.json` の探索と検証。`socket` / `logLevel` / `scrollback` / `rings` をデーモンが使う。
//! `keys` は値の型だけを検証して読み捨てる（使うのは CLI の attach を持つ後続の段）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use misao_daemon::{Limits, LogLevel};
use serde_json::{Map, Value};

use crate::socket_path::{DirOrigin, PathEnv, config_candidates_in_dirs};

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
}

#[derive(Debug)]
pub struct LoadedConfig {
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
        config: Config {
            socket: None,
            log_level: LogLevel::DEFAULT,
            limits: Limits::default(),
        },
        warnings: Vec::new(),
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
    let warnings = obj
        .keys()
        .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
        .map(|k| format!("{source}: unknown key \"{k}\" ignored"))
        .collect();
    let socket = obj.get("socket").and_then(Value::as_str).map(str::to_string);
    let log_level = obj
        .get("logLevel")
        .and_then(Value::as_str)
        .and_then(LogLevel::parse)
        .unwrap_or(LogLevel::DEFAULT);
    Ok(LoadedConfig {
        config: Config {
            socket,
            log_level,
            limits: read_limits(&obj),
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
    issues
}

const RING_KEYS: [&str; 3] = ["rawBytes", "linesBytes", "events"];

/// `rings` の中身。未知のキーは拒否し（Node 版の strictObject）、値は 1 以上の整数。
fn validate_rings(rings: &Map<String, Value>) -> Vec<String> {
    rings
        .iter()
        .filter_map(|(key, value)| {
            if !RING_KEYS.contains(&key.as_str()) {
                Some(format!("  rings: unrecognized key \"{key}\""))
            } else if !value.as_u64().is_some_and(|n| n >= 1) {
                Some(format!("  rings.{key}: expected an integer >= 1"))
            } else {
                None
            }
        })
        .collect()
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
        let err = load(&dir, None, Some(r#"{"rings":{"rawBytes":0,"bogus":1}}"#))
            .unwrap_err()
            .to_string();
        assert!(err.contains("rings.rawBytes: expected an integer >= 1"), "{err}");
        assert!(err.contains("rings: unrecognized key \"bogus\""), "{err}");
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
}
