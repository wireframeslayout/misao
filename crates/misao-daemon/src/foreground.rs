//! 前面プロセスのコマンド名（`pane.info` の `fgCommand`。Node 版の `foreground.ts`）。
//!
//! Linux は pty の前面プロセスグループのリーダーの `/proc/<pgid>/cmdline` から、他の Unix（macOS）は前面プロセスグループ
//! のリーダーの実行ファイル名（`ps`）から得る。取れなければ None（応答は落とさない）。
//!
//! Windows は未対応で、常に None を返す。ConPTY には前面プロセスグループが無く、実行中の子孫を誤りなく選ぶ手段が無いので、
//! 誤ったコマンド名を報告するより、報告しないことを選ぶ。

// argv の解釈は Linux だけが使う（他の Unix は `ps` が返す実行ファイル名をそのまま使う）。単体テストと差分テストは全環境で動かす。
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// 表示用の名前の上限（コードポイント数）。
const MAX_COMMAND_NAME_LENGTH: usize = 64;

/// インタプリタの引数の読み方。スクリプト名を探すときに飛ばす・止まる引数を決める。
struct InterpreterSyntax {
    /// 次の引数を値として取るオプション（`--opt=value` の形なら次の引数は飛ばさない）。
    value_options: &'static [&'static str],
    /// コードを直接渡すオプション。スクリプトが無いのでインタプリタ名を返す。
    code_options: &'static [&'static str],
    /// 次の引数のモジュール名をそのまま名前にするオプション（python -m）。
    module_options: &'static [&'static str],
    /// スクリプトの前に置くサブコマンド（bun run）。
    subcommands: &'static [&'static str],
}

const NODE: InterpreterSyntax = InterpreterSyntax {
    value_options: &[
        "-r",
        "--require",
        "--import",
        "--loader",
        "--experimental-loader",
        "-C",
        "--conditions",
        "--env-file",
        "--title",
    ],
    code_options: &["-e", "--eval", "-p", "--print"],
    module_options: &[],
    subcommands: &[],
};
const BUN: InterpreterSyntax = InterpreterSyntax {
    value_options: &[
        "-r",
        "--require",
        "--preload",
        "--import",
        "--conditions",
        "--cwd",
        "-c",
        "--config",
        "--env-file",
    ],
    code_options: &["-e", "--eval", "-p", "--print"],
    module_options: &[],
    subcommands: &["run", "x"],
};
const PYTHON: InterpreterSyntax = InterpreterSyntax {
    value_options: &["-W", "-X"],
    code_options: &["-c"],
    module_options: &["-m"],
    subcommands: &[],
};
const RUBY: InterpreterSyntax = InterpreterSyntax {
    value_options: &["-r", "-I", "-C"],
    code_options: &["-e"],
    module_options: &[],
    subcommands: &[],
};
const PERL: InterpreterSyntax = InterpreterSyntax {
    value_options: &[],
    code_options: &["-e", "-E"],
    module_options: &[],
    subcommands: &[],
};

/// 前面のコマンド名として実体ではなくスクリプト名を見せたいインタプリタ。シェルは含めない。
fn interpreter_syntax(command: &str) -> Option<&'static InterpreterSyntax> {
    match command {
        "node" | "nodejs" => Some(&NODE),
        "bun" => Some(&BUN),
        "ruby" => Some(&RUBY),
        "perl" => Some(&PERL),
        _ => is_python(command).then_some(&PYTHON),
    }
}

/// `python` / `python3` / `python3.12`。
fn is_python(command: &str) -> bool {
    let Some(version) = command.strip_prefix("python") else {
        return false;
    };
    match version.strip_prefix('3') {
        None => version.is_empty(),
        Some("") => true,
        Some(minor) => minor
            .strip_prefix('.')
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())),
    }
}

/// `/proc/<pid>/stat` から前面プロセスグループ ID（tpgid）を取り出す。取れなければ None。
pub fn parse_foreground_pgid(stat: &str) -> Option<u32> {
    // comm は括弧で囲まれ、中に ") (" などを含みうるので、最後の ")" より後ろを空白で分割する。
    // 括弧の後ろは state, ppid, pgrp, session, tty_nr, tpgid の順。
    let (_, rest) = stat.rsplit_once(')')?;
    let tpgid: i64 = rest.split_whitespace().nth(5)?.parse().ok()?;
    u32::try_from(tpgid).ok().filter(|&pgid| pgid > 0)
}

/// Unicode の書式文字（Cf）。bidi の制御・ゼロ幅の文字など、表示を乱すもの。
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{AD}' | '\u{600}'..='\u{605}' | '\u{61C}' | '\u{6DD}' | '\u{70F}' | '\u{890}'..='\u{891}' | '\u{8E2}' | '\u{180E}'
        | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}' | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}' | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}

/// 制御文字（Cc）・書式文字（Cf）・行区切り（Zl）・段落区切り（Zp）。端末や表に流れても表示を乱さないよう取り除く。
fn is_unsafe_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') || is_format_char(c)
}

/// 表示用に制御文字・書式文字・行/段落区切りを除き、64 コードポイントで切る。空になれば None。
pub fn sanitize_command_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|&c| !is_unsafe_char(c))
        .take(MAX_COMMAND_NAME_LENGTH)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// POSIX の `path.basename`（末尾の `/` は無視する）。
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// `path.parse(basename).name`（最後の `.` より前。先頭の `.` だけなら拡張子なし）。
fn script_basename(script: &str) -> &str {
    let base = basename(script);
    match base.rfind('.') {
        Some(i) if i > 0 && base != ".." => &base[..i],
        _ => base,
    }
}

/// argv から表示用のコマンド名を決める。インタプリタ経由ならスクリプト名（拡張子なし）かモジュール名。
pub fn command_name_from_argv(argv: &[String]) -> Option<String> {
    let first = argv.first()?;
    let command = basename(first);
    let name = match interpreter_syntax(command) {
        Some(syntax) => script_name(command, &argv[1..], syntax),
        None => command,
    };
    sanitize_command_name(name)
}

/// インタプリタの引数からスクリプト名を探す。スクリプトが無ければインタプリタ名。
fn script_name<'a>(command: &'a str, args: &'a [String], syntax: &InterpreterSyntax) -> &'a str {
    let mut saw_subcommand = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return args.get(i + 1).map_or(command, |script| script_basename(script));
        }
        // --opt=value の形は名前と値に分ける
        let (name, inline_value) = match arg.strip_prefix("--").and_then(|_| arg.split_once('=')) {
            Some((name, value)) => (name, Some(value)),
            None => (arg, None),
        };
        if syntax.code_options.contains(&name) {
            return command;
        }
        if syntax.module_options.contains(&name) {
            return inline_value
                .or_else(|| args.get(i + 1).map(String::as_str))
                .unwrap_or(command);
        }
        if syntax.value_options.contains(&name) {
            if inline_value.is_none() {
                i += 1;
            }
        } else if arg.starts_with('-') {
            // 値を取らないオプション
        } else if !saw_subcommand && syntax.subcommands.contains(&arg) {
            saw_subcommand = true;
        } else {
            return script_basename(arg);
        }
        i += 1;
    }
    command
}

/// 前面プロセスのコマンド名を知りたい pane。`pty_pgid` は pty の前面プロセスグループ（Linux 以外の Unix で使う。Linux は `/proc` から引く）。
#[derive(Debug, Clone, Copy)]
pub struct ForegroundQuery {
    pub pid: u32,
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub pty_pgid: Option<i32>,
}

/// 前面プロセスのコマンド名を、問い合わせの順に返す。取れなければ None。
/// Linux は pane ごとに `/proc` を読む。他の Unix は、まとめて `ps` を 1 回だけ起動する（pane が多い一覧で pane ごとに起動しない）。
pub fn read_foreground_commands(queries: &[ForegroundQuery]) -> Vec<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        queries.iter().map(|q| linux_foreground_command(q.pid)).collect()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        ps_foreground_commands(queries)
    }
    #[cfg(not(unix))]
    {
        vec![None; queries.len()]
    }
}

#[cfg(target_os = "linux")]
fn linux_foreground_command(pid: u32) -> Option<String> {
    let pgid = parse_foreground_pgid(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    let raw = std::fs::read(format!("/proc/{pgid}/cmdline")).ok()?;
    let mut argv: Vec<String> = raw
        .split(|&b| b == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    if argv.last().is_some_and(String::is_empty) {
        argv.pop();
    }
    command_name_from_argv(&argv)
}

/// `ps -o pid=,comm= -p a,b,c` の出力から pid -> 実行ファイル名の表を作る。comm は空白を含みうるので、最初の空白で分ける。
#[cfg(any(all(unix, not(target_os = "linux")), test))]
fn parse_ps_output(output: &str) -> std::collections::HashMap<i32, String> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, comm) = line.trim().split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, sanitize_command_name(basename(comm.trim()))?))
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn ps_foreground_commands(queries: &[ForegroundQuery]) -> Vec<Option<String>> {
    let pgids: Vec<i32> = queries
        .iter()
        .filter_map(|q| q.pty_pgid.filter(|&p| p > 0))
        .collect();
    if pgids.is_empty() {
        return vec![None; queries.len()];
    }
    let list = pgids.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    let names = std::process::Command::new("/bin/ps")
        .args(["-o", "pid=,comm=", "-p", &list])
        .output()
        .ok()
        .filter(|output| output.status.success() || !output.stdout.is_empty())
        .map(|output| parse_ps_output(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    queries
        .iter()
        .map(|q| q.pty_pgid.and_then(|pgid| names.get(&pgid).cloned()))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn python_command_names() {
        for yes in ["python", "python3", "python3.12"] {
            assert!(is_python(yes), "{yes}");
        }
        for no in [
            "python2",
            "python3.",
            "python3.x",
            "pythonw",
            "Python3",
            "python33",
        ] {
            assert!(!is_python(no), "{no}");
        }
    }

    #[test]
    fn ps_output_is_parsed_per_pid_and_keeps_names_with_spaces() {
        let names = parse_ps_output("  101 /usr/bin/vim\n  102 /Applications/My App/tool\nbad line\n  x y\n");
        assert_eq!(names.get(&101).map(String::as_str), Some("vim"));
        assert_eq!(names.get(&102).map(String::as_str), Some("tool"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn an_empty_command_has_no_name() {
        assert_eq!(command_name_from_argv(&argv(&[""])), None);
        assert_eq!(command_name_from_argv(&[]), None);
    }
}
