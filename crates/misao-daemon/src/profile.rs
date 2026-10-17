//! エージェント固有の画面規則（Node 版の `AgentProfile`）。Node 版は classify をコードで書く差し込み口だけで、
//! Claude / Codex の規則は同梱していなかった。Rust 版は規則をデータ（`profiles/*.json`）として持ち、
//! 読み込み時に検証して、判定は純粋な関数として評価する。
//!
//! 画面は Claude Code の入力欄（`─` の罫線 2 本とその間の `❯`）で「上 / 入力欄 / 下」に分け、規則は読む範囲
//! （`region`）を名指しする。規則は優先順位が最も高く一致したものが勝つ（同順位なら先に書いたもの）。
//! `working` / `blocked` / `idle` はそのまま意見になる。`skip`（画面が別の表示に切り替わっている）と
//! `error`（API エラーで止まっている）は意見なし（None）にするが、競り合いには加わるので、下位の規則を黙らせる。

use std::sync::Arc;

use serde::Deserialize;

use crate::profile_rules::{RawRule, Region, RegionLines, Rule, RuleOutcome};

const CLAUDE: &str = include_str!("profiles/claude.json");
const CODEX: &str = include_str!("profiles/codex.json");
/// 入力欄の上に残す行数の上限。
const MAX_ABOVE_LINES: usize = 12;
/// 罫線とみなす `─` の最小の長さ。
const MIN_RULE_LEN: usize = 10;

/// 規則が判定した結果（Node 版の `ProfileVerdict` の null 以外）。blocked を出せるのはプロファイルだけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileVerdict {
    Working,
    Blocked,
    Idle,
}

/// プロファイルが画面を判定するための入力。
pub struct ProfileScreen {
    /// 見えている各行（右端の空白は除去済み）。
    pub rows: Vec<String>,
    pub title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    name: String,
    /// 起動コマンドの実行ファイル名（`env` ラッパーと拡張子 `.exe` は除く）。
    programs: Vec<String>,
    rules: Vec<RawRule>,
}

pub struct Profile {
    /// 判定したとき `pane.state` の `decidedBy` に載る。
    pub name: String,
    programs: Vec<String>,
    rules: Vec<Rule>,
}

/// 画面を「上 / 入力欄 / 下」に分けたもの。
struct PromptBoxSplit {
    above: Vec<String>,
    prompt_box: Vec<String>,
    below: Vec<String>,
}

fn is_rule_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= MIN_RULE_LEN && trimmed.chars().all(|c| c == '─')
}

fn non_blank(rows: &[String]) -> impl Iterator<Item = &String> {
    rows.iter().filter(|l| !l.trim().is_empty())
}

/// 下から 2 本目までの罫線で入力欄を囲む。罫線が 2 本無ければ、入力欄なしで末尾の空でない行だけを「上」にする。
fn split_prompt_box(rows: &[String]) -> PromptBoxSplit {
    let mut rules = (0..rows.len()).rev().filter(|&i| is_rule_line(&rows[i]));
    let (Some(bottom), Some(top)) = (rules.next(), rules.next()) else {
        let kept: Vec<&String> = non_blank(rows).collect();
        return PromptBoxSplit {
            above: kept[kept.len().saturating_sub(MAX_ABOVE_LINES)..]
                .iter()
                .map(|l| (*l).clone())
                .collect(),
            prompt_box: Vec::new(),
            below: Vec::new(),
        };
    };
    let above: Vec<&String> = non_blank(&rows[..top]).collect();
    PromptBoxSplit {
        above: above[above.len().saturating_sub(MAX_ABOVE_LINES)..]
            .iter()
            .map(|l| (*l).clone())
            .collect(),
        prompt_box: rows[top + 1..bottom].to_vec(),
        below: non_blank(&rows[bottom + 1..]).cloned().collect(),
    }
}

impl RawProfile {
    fn compile(self) -> Result<Profile, String> {
        let name = self.name;
        let rules = self
            .rules
            .into_iter()
            .map(|raw| {
                let when = raw
                    .when
                    .compile()
                    .map_err(|e| format!("profile {name}, rule {}: {e}", raw.id))?;
                Ok(Rule {
                    outcome: raw.verdict,
                    priority: raw.priority,
                    region: raw.region,
                    when,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Profile {
            name,
            programs: self.programs,
            rules,
        })
    }
}

impl Profile {
    /// 画面を判定する。意見がなければ None（title / bytes 段に任せる）。純粋関数。
    pub fn classify(&self, screen: &ProfileScreen) -> Option<ProfileVerdict> {
        let split = split_prompt_box(&screen.rows);
        let all: Vec<String> = [&split.above, &split.prompt_box, &split.below]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        let title: Vec<String> = if screen.title.is_empty() {
            Vec::new()
        } else {
            vec![screen.title.clone()]
        };
        // 小文字にした行は、範囲ごとに 1 度だけ作る（規則ごとに作り直さない）。
        let lower = |lines: &[String]| lines.iter().map(|l| l.to_lowercase()).collect::<Vec<_>>();
        let (above_l, box_l, below_l, all_l, title_l) = (
            lower(&split.above),
            lower(&split.prompt_box),
            lower(&split.below),
            lower(&all),
            lower(&title),
        );
        let above = RegionLines::new(&split.above, &above_l);
        let prompt_box = RegionLines::new(&split.prompt_box, &box_l);
        let below = RegionLines::new(&split.below, &below_l);
        let all = RegionLines::new(&all, &all_l);
        let title = RegionLines::new(&title, &title_l);
        let mut best: Option<&Rule> = None;
        for rule in &self.rules {
            let region = match rule.region {
                Region::Above => &above,
                Region::PromptBox => &prompt_box,
                Region::Below => &below,
                Region::All => &all,
                Region::Title => &title,
            };
            if region.is_empty() || best.is_some_and(|b| b.priority >= rule.priority) {
                continue;
            }
            if rule.when.holds(region) {
                best = Some(rule);
            }
        }
        match best?.outcome {
            RuleOutcome::Working => Some(ProfileVerdict::Working),
            RuleOutcome::Blocked => Some(ProfileVerdict::Blocked),
            RuleOutcome::Idle => Some(ProfileVerdict::Idle),
            RuleOutcome::Skip | RuleOutcome::Error => None,
        }
    }

    fn runs(&self, program: &str) -> bool {
        self.programs.iter().any(|p| p == program)
    }
}

/// パスの最後の要素（`/` と `\` で区切る）から、拡張子 `.exe`（大文字小文字を問わない）を除いたもの。
fn base_name(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem_len = name.len().checked_sub(4).filter(|&i| name.is_char_boundary(i));
    match stem_len {
        Some(i) if name[i..].eq_ignore_ascii_case(".exe") => name[..i].to_string(),
        _ => name.to_string(),
    }
}

/// `cmd` の実行ファイル名。先頭が `env` なら、そのオプションと `NAME=VALUE` を飛ばした次の語を見る。
fn program_name(cmd: &[String]) -> String {
    let mut words = cmd.iter();
    let Some(first) = words.next() else {
        return String::new();
    };
    if base_name(first) != "env" {
        return base_name(first);
    }
    while let Some(word) = words.next() {
        match word.as_str() {
            "-u" | "--unset" | "-C" | "--chdir" => {
                words.next();
            }
            w if w.starts_with('-') || is_assignment(w) => {}
            w => return base_name(w),
        }
    }
    String::new()
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];
/// 先頭に付いても、その次の語が本体のコマンドであるもの。
const COMMAND_PREFIXES: [&str; 4] = ["exec", "command", "nohup", "env"];

/// 起動コマンドが実行しようとしているプログラム名。`sh -lc "cd d && claude --x"` のようにシェルへ文字列で渡すときは、
/// その文字列の各コマンド（`&&` `||` `;` `|` `&` で区切った先頭の語）。そうでなければ `program_name` の 1 つ。
fn command_programs(cmd: &[String]) -> Vec<String> {
    let launcher = program_name(cmd);
    if !SHELLS.contains(&launcher.as_str()) {
        return vec![launcher];
    }
    let script = cmd.iter().enumerate().skip(1).find_map(|(i, arg)| {
        let is_command_flag = arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c');
        is_command_flag.then(|| cmd.get(i + 1)).flatten()
    });
    let Some(script) = script else {
        return vec![launcher];
    };
    script
        .split(['&', '|', ';', '\n'])
        .filter_map(|segment| {
            let word = segment
                .split_whitespace()
                .find(|w| !is_assignment(w) && !COMMAND_PREFIXES.contains(w) && !w.starts_with('-'))?;
            Some(base_name(word.trim_matches(['\'', '"'])))
        })
        .collect()
}

/// pane に使うプロファイル。前面プロセス名（`fg`）が分かればそれで選ぶ（シェルの中で後から始めたエージェントも拾う）。
/// 前面が分からないとき、起動コマンドがシェル以外のとき、シェルがシェルのままのときは、起動コマンドが実行しようとしたプログラムで選ぶ。
/// シェルの前面が別のプログラムなら、プロファイルなし。
pub fn select_profile_for(
    profiles: &[Arc<Profile>],
    cmd: &[String],
    fg: Option<&str>,
) -> Option<Arc<Profile>> {
    if let Some(name) = fg
        && let Some(profile) = profiles.iter().find(|p| p.runs(name))
    {
        return Some(Arc::clone(profile));
    }
    // シェルの前面が別のプログラムなら、シェルが実行しようとしたものではない（たとえば claude が終わって git が前面）。
    // シェル以外が起動コマンドなら（claude 自身や、sh のラッパースクリプト）、前面の名前が違っても起動コマンドで選ぶ。
    let launcher = program_name(cmd);
    if SHELLS.contains(&launcher.as_str()) && fg.is_some_and(|name| name != launcher) {
        return None;
    }
    let programs = command_programs(cmd);
    profiles
        .iter()
        .find(|p| programs.iter().any(|program| p.runs(program)))
        .cloned()
}

/// 同梱のプロファイル（Claude Code / Codex）。
pub fn builtin_profiles() -> Result<Vec<Arc<Profile>>, String> {
    [CLAUDE, CODEX]
        .into_iter()
        .map(|text| {
            let raw: RawProfile =
                serde_json::from_str(text).map_err(|e| format!("invalid profile data: {e}"))?;
            raw.compile().map(Arc::new)
        })
        .collect()
}

/// 前面のプログラムを見ても選択が変わりうるか。起動コマンドがシェル以外で、起動コマンドだけで profile が決まるなら、
/// 前面を調べ続ける必要はない（調べない）。profile が無ければ、そもそも選ぶものがない。
pub fn selection_can_change(profiles: &[Arc<Profile>], cmd: &[String]) -> bool {
    if profiles.is_empty() {
        return false;
    }
    let is_shell = SHELLS.contains(&program_name(cmd).as_str());
    is_shell || select_profile(profiles, cmd).is_none()
}

/// 起動コマンドだけで選ぶ（前面プロセスが分かる前の最初の選択）。
pub fn select_profile(profiles: &[Arc<Profile>], cmd: &[String]) -> Option<Arc<Profile>> {
    select_profile_for(profiles, cmd, None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn profile(name: &str) -> Arc<Profile> {
        builtin_profiles()
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
    }

    fn screen(rows: &[&str], title: &str) -> ProfileScreen {
        ProfileScreen {
            rows: rows.iter().map(|r| (*r).to_string()).collect(),
            title: title.to_string(),
        }
    }

    fn cmd(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    const RULE: &str = "────────────────────────────────";

    #[test]
    fn the_builtin_data_loads_and_selects_by_program_name() {
        let profiles = builtin_profiles().unwrap();
        let pick = |c: &[&str]| select_profile(&profiles, &cmd(c)).map(|p| p.name.clone());
        assert_eq!(pick(&["claude"]), Some("claude".into()));
        assert_eq!(pick(&["/usr/local/bin/codex", "--x"]), Some("codex".into()));
        assert_eq!(
            pick(&["env", "-u", "CLAUDE_X", "FOO=1", "claude"]),
            Some("claude".into())
        );
        assert_eq!(pick(&["C:\\bin\\claude.EXE"]), Some("claude".into()));
        assert_eq!(pick(&["bash"]), None);
        assert_eq!(pick(&["env"]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn shell_wrapped_commands_are_judged_by_the_program_they_run() {
        let profiles = builtin_profiles().unwrap();
        let pick =
            |c: &[&str], fg: Option<&str>| select_profile_for(&profiles, &cmd(c), fg).map(|p| p.name.clone());
        assert_eq!(
            pick(&["/bin/zsh", "-lc", "claude --model x"], None),
            Some("claude".into())
        );
        assert_eq!(
            pick(&["bash", "-c", "cd /w && ANTHROPIC=1 exec codex 'a b'"], None),
            Some("codex".into())
        );
        assert_eq!(
            pick(&["sh", "-c", "ls | grep x; claude"], None),
            Some("claude".into())
        );
        assert_eq!(
            pick(&["zsh", "-lc", "claude"], Some("zsh")),
            Some("claude".into())
        );
        assert_eq!(pick(&["zsh", "-lc", "claude"], Some("git")), None);
        assert_eq!(pick(&["zsh", "-l"], None), None);
        assert_eq!(pick(&["zsh", "-i"], Some("claude")), Some("claude".into()));
        assert_eq!(pick(&["zsh", "-i"], Some("zsh")), None);
        assert_eq!(pick(&["claude"], Some("node")), Some("claude".into()));
        assert_eq!(pick(&["python3", "-c", "claude"], None), None);
    }

    #[test]
    fn only_panes_whose_profile_can_change_are_probed() {
        let profiles = builtin_profiles().unwrap();
        let probed = |c: &[&str]| selection_can_change(&profiles, &cmd(c));
        assert!(probed(&["zsh", "-i"]));
        assert!(probed(&["zsh", "-lc", "claude"]));
        assert!(probed(&["node", "tool.js"]));
        assert!(!probed(&["claude", "--x"]));
        assert!(!probed(&["env", "A=1", "codex"]));
        assert!(!selection_can_change(&[], &cmd(&["zsh"])));
    }

    #[test]
    fn claude_reads_the_prompt_box_and_its_surroundings() {
        let claude = profile("claude");
        let idle = screen(&["hello", RULE, "❯ ", RULE, "  ? for shortcuts"], "");
        assert_eq!(claude.classify(&idle), Some(ProfileVerdict::Idle));
        let working = screen(&["✻ Pondering… (3s · esc to interrupt)", RULE, "❯ ", RULE], "");
        assert_eq!(claude.classify(&working), Some(ProfileVerdict::Working));
        let blocked = screen(
            &["⏺ Bash(ls)", RULE, " Do you want to proceed?", " ❯ 1. Yes", RULE],
            "",
        );
        assert_eq!(claude.classify(&blocked), Some(ProfileVerdict::Blocked));
        assert_eq!(claude.classify(&screen(&["plain output"], "")), None);
    }

    #[test]
    fn a_skip_or_error_rule_silences_lower_priority_rules_without_giving_an_opinion() {
        let claude = profile("claude");
        let viewer = screen(&["Do you want to proceed?", "Showing detailed transcript"], "");
        assert_eq!(claude.classify(&viewer), None);
        let error = screen(&["API Error: request timed out", RULE, "❯ ", RULE], "");
        assert_eq!(claude.classify(&error), None);
        let retrying = screen(
            &["API Error: request timed out, retrying in 4s", RULE, "❯ ", RULE],
            "",
        );
        assert_eq!(claude.classify(&retrying), Some(ProfileVerdict::Idle));
    }

    #[test]
    fn codex_reads_the_title_and_the_last_lines() {
        let codex = profile("codex");
        assert_eq!(
            codex.classify(&screen(&["x"], "Action Required - codex")),
            Some(ProfileVerdict::Blocked)
        );
        assert_eq!(
            codex.classify(&screen(&["Allow command? [y/n]"], "codex")),
            Some(ProfileVerdict::Blocked)
        );
        assert_eq!(
            codex.classify(&screen(&["Working (3s • esc to interrupt)"], "codex")),
            Some(ProfileVerdict::Working)
        );
        assert_eq!(codex.classify(&screen(&["idle"], "codex")), None);
    }

    #[test]
    fn invalid_data_is_rejected_when_loading() {
        let bad = r#"{"name":"x","programs":[],"rules":[{"id":"r","verdict":"idle","priority":1,"region":"all","when":{"matches":"("}}]}"#;
        let raw: RawProfile = serde_json::from_str(bad).unwrap();
        let error = raw.compile().err().unwrap();
        assert!(error.contains("profile x, rule r"), "{error}");
        assert!(
            serde_json::from_str::<RawProfile>(r#"{"name":"x","programs":[],"rules":[],"extra":1}"#).is_err()
        );
    }
}
