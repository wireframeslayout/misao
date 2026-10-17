//! 引数の解釈（Node 版 `args.ts` と `util.parseArgs` の strict モード相当）。
//! 日本語のエラーと終了コード 2 を Node 版に合わせるため、clap の自動エラーには頼らず自前で解釈する。

use std::collections::HashMap;

use crate::error::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Str,
}

#[derive(Debug, Clone, Copy)]
pub struct OptSpec {
    pub name: &'static str,
    pub kind: Kind,
    pub multiple: bool,
}

pub const fn flag(name: &'static str) -> OptSpec {
    OptSpec {
        name,
        kind: Kind::Bool,
        multiple: false,
    }
}

pub const fn string(name: &'static str) -> OptSpec {
    OptSpec {
        name,
        kind: Kind::Str,
        multiple: false,
    }
}

pub const fn strings(name: &'static str) -> OptSpec {
    OptSpec {
        name,
        kind: Kind::Str,
        multiple: true,
    }
}

/// どのコマンドでも受け付ける。
pub const GLOBAL_OPTIONS: [OptSpec; 4] = [string("config"), flag("json"), flag("help"), flag("version")];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Flag,
    One(String),
    Many(Vec<String>),
}

/// 解析済みの引数。値の取り出しで型と範囲を検証する（境界での入力検証）。
#[derive(Debug, Clone, Default)]
pub struct ParsedArgs {
    /// `--` より前の位置引数。
    pub positionals: Vec<String>,
    /// `--` より後ろ。
    pub rest: Vec<String>,
    values: HashMap<String, Value>,
}

impl ParsedArgs {
    pub fn flag(&self, name: &str) -> bool {
        self.values.get(name) == Some(&Value::Flag)
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.values.get(name) {
            Some(Value::One(v)) => Some(v),
            Some(Value::Many(vs)) => vs.last().map(String::as_str),
            _ => None,
        }
    }

    pub fn strings(&self, name: &str) -> Vec<String> {
        match self.values.get(name) {
            Some(Value::Many(vs)) => vs.clone(),
            Some(Value::One(v)) => vec![v.clone()],
            _ => Vec::new(),
        }
    }

    /// 整数オプション。min 未満や数字以外は usage エラー。
    pub fn int(&self, name: &str, min: u64) -> Result<Option<u64>, CliError> {
        let Some(raw) = self.string(name) else {
            return Ok(None);
        };
        let bad = || CliError::usage(format!("--{name} には {min} 以上の整数を指定してください: {raw}"));
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        match raw.parse::<u64>() {
            Ok(n) if n >= min => Ok(Some(n)),
            _ => Err(bad()),
        }
    }
}

fn usage_error(detail: &str) -> CliError {
    CliError::usage(format!("引数が正しくありません: {detail}"))
}

/// `util.parseArgs` の未知のオプションの文面。短いオプション(`-abc`)は先頭の 1 文字だけを示す。
fn unknown_option(token: &str) -> CliError {
    let shown: String = if token.starts_with("--") {
        token.to_string()
    } else {
        token.chars().take(2).collect()
    };
    usage_error(&format!(
        "Unknown option '{shown}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{shown}\""
    ))
}

fn looks_like_option(value: &str) -> bool {
    value.chars().count() > 1 && value.starts_with('-')
}

/// 厳密に解釈する（未知のオプション・値の欠落・値の曖昧さは usage エラー）。
pub fn parse_command_args(argv: &[String], specs: &[OptSpec]) -> Result<ParsedArgs, CliError> {
    let find = |name: &str| {
        specs
            .iter()
            .chain(GLOBAL_OPTIONS.iter())
            .find(|s| s.name == name)
            .copied()
    };
    let mut parsed = ParsedArgs::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg == "--" {
            parsed.rest = argv[i..].to_vec();
            break;
        }
        let Some(body) = arg.strip_prefix("--") else {
            if looks_like_option(arg) {
                return Err(unknown_option(arg));
            }
            parsed.positionals.push(arg.clone());
            continue;
        };
        let (name, inline) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        let Some(spec) = find(name) else {
            return Err(unknown_option(&format!("--{name}")));
        };
        match spec.kind {
            Kind::Bool => {
                if inline.is_some() {
                    return Err(usage_error(&format!(
                        "Option '--{name}' does not take an argument"
                    )));
                }
                parsed.values.insert(name.to_string(), Value::Flag);
            }
            Kind::Str => {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        let next = argv.get(i).ok_or_else(|| {
                            usage_error(&format!("Option '--{name} <value>' argument missing"))
                        })?;
                        if looks_like_option(next) {
                            return Err(usage_error(&format!(
                                "Option '--{name}' argument is ambiguous.\nDid you forget to specify the option argument for '--{name}'?\nTo specify an option argument starting with a dash use '--{name}=-XYZ'."
                            )));
                        }
                        i += 1;
                        next.clone()
                    }
                };
                store(&mut parsed.values, name, value, spec.multiple);
            }
        }
    }
    Ok(parsed)
}

fn store(values: &mut HashMap<String, Value>, name: &str, value: String, multiple: bool) {
    if multiple {
        match values
            .entry(name.to_string())
            .or_insert_with(|| Value::Many(Vec::new()))
        {
            Value::Many(vs) => vs.push(value),
            other => *other = Value::Many(vec![value]),
        }
    } else {
        values.insert(name.to_string(), Value::One(value));
    }
}

pub struct SplitArgs {
    /// 最初の位置引数。無ければ None。
    pub command: Option<String>,
    /// command を除いた引数。
    pub args: Vec<String>,
    /// コマンド名より前後どこにあっても読めるグローバルオプション。
    pub globals: ParsedArgs,
}

/// コマンド名とグローバルオプションを先に取り出す（コマンド固有のオプションはまだ解釈しない。`strict: false` 相当）。
pub fn split_command(argv: &[String]) -> SplitArgs {
    let mut globals = ParsedArgs::default();
    let mut first: Option<usize> = None;
    let mut is_after_terminator = false;
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        let at = i;
        i += 1;
        if is_after_terminator || !arg.starts_with('-') || arg == "-" {
            first.get_or_insert(at);
            continue;
        }
        if arg == "--" {
            is_after_terminator = true;
            continue;
        }
        let Some(body) = arg.strip_prefix("--") else {
            continue;
        };
        let (name, inline) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        let Some(spec) = GLOBAL_OPTIONS.iter().find(|s| s.name == name) else {
            continue;
        };
        match spec.kind {
            Kind::Bool => {
                globals.values.insert(name.to_string(), Value::Flag);
            }
            Kind::Str => {
                let value = inline.or_else(|| {
                    let next = argv.get(i).cloned();
                    if next.is_some() {
                        i += 1;
                    }
                    next
                });
                if let Some(value) = value {
                    store(&mut globals.values, name, value, false);
                }
            }
        }
    }
    let command = first.map(|at| argv[at].clone());
    let args = argv
        .iter()
        .enumerate()
        .filter(|(at, _)| Some(*at) != first)
        .map(|(_, a)| a.clone())
        .collect();
    SplitArgs {
        command,
        args,
        globals,
    }
}

/// `k=v` を (k, v) に分ける。= が無い・k が空なら usage エラー。値は空でもよい。
fn parse_key_value(item: &str, option_name: &str) -> Result<(String, String), CliError> {
    match item.find('=') {
        Some(at) if at >= 1 => Ok((item[..at].to_string(), item[at + 1..].to_string())),
        _ => Err(CliError::usage(format!(
            "{option_name} は k=v の形で指定してください: {item}"
        ))),
    }
}

/// 同じキーを複数回書いたときは後のものが勝つ。
pub fn parse_key_values(items: &[String], option_name: &str) -> Result<Vec<(String, String)>, CliError> {
    let mut out: Vec<(String, String)> = Vec::new();
    for item in items {
        let (k, v) = parse_key_value(item, option_name)?;
        match out.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => out.push((k, v)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    const SPECS: [OptSpec; 4] = [string("state"), strings("label"), flag("force"), string("since")];

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn split_reads_globals_before_and_after_the_command() {
        let a = split_command(&v(&["--json", "ls", "--state", "blocked"]));
        assert_eq!(a.command.as_deref(), Some("ls"));
        assert_eq!(a.args, v(&["--json", "--state", "blocked"]));
        assert!(a.globals.flag("json"));
        let b = split_command(&v(&["--config", "/x.json", "status"]));
        assert_eq!(b.command.as_deref(), Some("status"));
        assert_eq!(b.globals.string("config"), Some("/x.json"));
        assert_eq!(b.args, v(&["--config", "/x.json"]));
    }

    #[test]
    fn split_without_a_command_and_with_version_or_help() {
        assert_eq!(split_command(&[]).command, None);
        assert!(split_command(&v(&["--version"])).globals.flag("version"));
        assert!(split_command(&v(&["--help"])).globals.flag("help"));
    }

    #[test]
    fn version_after_the_terminator_belongs_to_the_command() {
        let s = split_command(&v(&["new", "--", "claude", "--version"]));
        assert_eq!(s.command.as_deref(), Some("new"));
        assert!(!s.globals.flag("version"));
    }

    #[test]
    fn parse_reads_positionals_values_multiples_and_flags() {
        let p = parse_command_args(
            &v(&[
                "abc", "--label", "a=1", "--label", "b=2", "--force", "--state", "idle",
            ]),
            &SPECS,
        )
        .unwrap();
        assert_eq!(p.positionals, v(&["abc"]));
        assert_eq!(p.strings("label"), v(&["a=1", "b=2"]));
        assert!(p.flag("force"));
        assert_eq!(p.string("state"), Some("idle"));
        assert_eq!(p.string("since"), None);
        assert!(p.strings("nothing").is_empty());
    }

    #[test]
    fn everything_after_the_terminator_goes_to_rest() {
        let p =
            parse_command_args(&v(&["--label", "x=1", "--", "claude", "--force", "a b"]), &SPECS).unwrap();
        assert_eq!(p.rest, v(&["claude", "--force", "a b"]));
        assert!(p.positionals.is_empty());
        assert!(!p.flag("force"));
    }

    #[test]
    fn unknown_options_and_missing_values_are_usage_errors() {
        for argv in [
            &["--nope"][..],
            &["--state"],
            &["--state", "--force"],
            &["-x"],
            &["--force=1"],
        ] {
            let e = parse_command_args(&v(argv), &SPECS).unwrap_err();
            assert_eq!((e.kind, e.exit_code()), (ErrorKind::Usage, 2), "{argv:?}");
        }
        assert!(parse_command_args(&v(&["--state=--x"]), &SPECS).is_ok());
    }

    #[test]
    fn messages_match_util_parse_args_exactly() {
        let message = |argv: &[&str]| parse_command_args(&v(argv), &SPECS).unwrap_err().message;
        let unknown = |shown: &str| {
            format!(
                "引数が正しくありません: Unknown option '{shown}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{shown}\""
            )
        };
        assert_eq!(message(&["--nope"]), unknown("--nope"));
        assert_eq!(message(&["--nope=1"]), unknown("--nope"));
        assert_eq!(message(&["-abc"]), unknown("-a"));
        assert_eq!(
            message(&["--state", "-1"]),
            "引数が正しくありません: Option '--state' argument is ambiguous.\nDid you forget to specify the option argument for '--state'?\nTo specify an option argument starting with a dash use '--state=-XYZ'."
        );
        assert_eq!(
            message(&["--state"]),
            "引数が正しくありません: Option '--state <value>' argument missing"
        );
        assert_eq!(
            message(&["--force=1"]),
            "引数が正しくありません: Option '--force' does not take an argument"
        );
    }

    #[test]
    fn int_validates_integers_and_the_minimum() {
        let p = |argv: &[&str]| parse_command_args(&v(argv), &SPECS).unwrap();
        assert_eq!(p(&["--since", "0"]).int("since", 0).unwrap(), Some(0));
        assert_eq!(p(&[]).int("since", 0).unwrap(), None);
        for bad in ["1.5", "abc", "", "+1"] {
            let args = parse_command_args(&v(&["--since", bad]), &SPECS).unwrap();
            assert!(args.int("since", 0).is_err(), "{bad:?}");
        }
        assert!(parse_command_args(&v(&["--since", "-1"]), &SPECS).is_err());
        assert!(p(&["--since", "0"]).int("since", 1).is_err());
    }

    #[test]
    fn key_values_split_at_the_first_equals_and_the_last_wins() {
        let items = v(&["a=1", "b=", "a=2", "c=x=y"]);
        assert_eq!(
            parse_key_values(&items, "--label").unwrap(),
            vec![
                ("a".into(), "2".into()),
                ("b".into(), String::new()),
                ("c".into(), "x=y".into())
            ]
        );
        assert!(parse_key_values(&v(&["novalue"]), "--label").is_err());
        assert!(parse_key_values(&v(&["=v"]), "--label").is_err());
    }
}
