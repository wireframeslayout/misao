//! エージェント固有の画面規則（Node 版の `AgentProfile`）。Node 版は classify をコードで書く差し込み口だけで、
//! Claude / Codex の規則は同梱していなかった。Rust 版は規則をデータ（`profiles/*.json`）として持ち、
//! 読み込み時に検証して、判定は純粋な関数として評価する。
//!
//! 画面は Claude Code の入力欄（`─` の罫線 2 本とその間の `❯`）で「上 / 入力欄 / 下」に分け、規則は読む範囲
//! （`region`）を名指しする。規則は優先順位が最も高く一致したものが勝つ（同順位なら先に書いたもの）。
//! `working` / `blocked` / `idle` はそのまま意見になる。`skip`（画面が別の表示に切り替わっている）と
//! `error`（API エラーで止まっている）は意見なし（None）にするが、競り合いには加わるので、下位の規則を黙らせる。

use std::sync::Arc;

use regex::Regex;
use serde::Deserialize;

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

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuleOutcome {
    Working,
    Blocked,
    Idle,
    Skip,
    Error,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Region {
    Above,
    PromptBox,
    Below,
    All,
    Title,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct LineMatch {
    regex: String,
    #[serde(default)]
    ignore_case: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct LastLines {
    n: usize,
    when: Box<RawCondition>,
}

/// 読み込んだままの条件（正規表現は文字列）。
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawCondition {
    All(Vec<RawCondition>),
    Any(Vec<RawCondition>),
    Not(Box<RawCondition>),
    /// 範囲の行を小文字にして改行でつないだ文字列が、この（小文字の）文字列を含む。
    Contains(String),
    /// 同じ文字列が、この正規表現に一致する。
    Matches(String),
    /// どれかの行（元の大文字小文字のまま）がこの正規表現に一致する。
    LineMatches(LineMatch),
    /// 範囲の末尾 n 行だけで評価する。
    LastLines(LastLines),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    verdict: RuleOutcome,
    priority: u32,
    region: Region,
    when: RawCondition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    name: String,
    /// 起動コマンドの実行ファイル名（`env` ラッパーと拡張子 `.exe` は除く）。
    programs: Vec<String>,
    rules: Vec<RawRule>,
}

enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    Contains(String),
    Matches(Regex),
    LineMatches(Regex),
    LastLines(usize, Box<Condition>),
}

impl RawCondition {
    fn compile(self) -> Result<Condition, String> {
        let regex = |pattern: &str, ignore_case: bool| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .build()
                .map_err(|e| format!("invalid regex {pattern:?}: {e}"))
        };
        let all = |list: Vec<Self>| list.into_iter().map(Self::compile).collect::<Result<Vec<_>, _>>();
        Ok(match self {
            Self::All(list) => Condition::All(all(list)?),
            Self::Any(list) => Condition::Any(all(list)?),
            Self::Not(inner) => Condition::Not(Box::new(inner.compile()?)),
            Self::Contains(needle) => Condition::Contains(needle.to_lowercase()),
            Self::Matches(pattern) => Condition::Matches(regex(&pattern, false)?),
            Self::LineMatches(LineMatch {
                regex: pattern,
                ignore_case,
            }) => Condition::LineMatches(regex(&pattern, ignore_case)?),
            Self::LastLines(LastLines { n, when }) => Condition::LastLines(n, Box::new(when.compile()?)),
        })
    }
}

impl Condition {
    fn holds(&self, lines: &[String]) -> bool {
        match self {
            Self::All(list) => list.iter().all(|c| c.holds(lines)),
            Self::Any(list) => list.iter().any(|c| c.holds(lines)),
            Self::Not(inner) => !inner.holds(lines),
            Self::Contains(needle) => joined_lowercase(lines).contains(needle.as_str()),
            Self::Matches(re) => re.is_match(&joined_lowercase(lines)),
            Self::LineMatches(re) => lines.iter().any(|line| re.is_match(line)),
            Self::LastLines(n, inner) => inner.holds(&lines[lines.len().saturating_sub(*n)..]),
        }
    }
}

fn joined_lowercase(lines: &[String]) -> String {
    lines
        .iter()
        .map(|l| l.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n")
}

struct Rule {
    outcome: RuleOutcome,
    priority: u32,
    region: Region,
    when: Condition,
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
        let mut best: Option<&Rule> = None;
        for rule in &self.rules {
            let lines = match rule.region {
                Region::Above => &split.above,
                Region::PromptBox => &split.prompt_box,
                Region::Below => &split.below,
                Region::All => &all,
                Region::Title => &title,
            };
            if lines.is_empty() || best.is_some_and(|b| b.priority >= rule.priority) {
                continue;
            }
            if rule.when.holds(lines) {
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

    fn matches(&self, cmd: &[String]) -> bool {
        let program = program_name(cmd);
        self.programs.contains(&program)
    }
}

/// `cmd` の実行ファイル名。先頭が `env` なら、そのオプションと `NAME=VALUE` を飛ばした次の語を見る。
fn program_name(cmd: &[String]) -> String {
    let base = |s: &str| {
        let name = s.rsplit(['/', '\\']).next().unwrap_or_default();
        name.strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".EXE"))
            .unwrap_or(name)
            .to_string()
    };
    let mut words = cmd.iter();
    let Some(first) = words.next() else {
        return String::new();
    };
    if base(first) != "env" {
        return base(first);
    }
    let is_assignment = |w: &str| {
        w.split_once('=').is_some_and(|(name, _)| {
            name.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    while let Some(word) = words.next() {
        match word.as_str() {
            "-u" | "--unset" | "-C" | "--chdir" => {
                words.next();
            }
            w if w.starts_with('-') || is_assignment(w) => {}
            w => return base(w),
        }
    }
    String::new()
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

/// cmd に最初に matches したプロファイル。
pub fn select_profile(profiles: &[Arc<Profile>], cmd: &[String]) -> Option<Arc<Profile>> {
    profiles.iter().find(|p| p.matches(cmd)).cloned()
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
        assert_eq!(pick(&["C:\\bin\\claude.exe"]), Some("claude".into()));
        assert_eq!(pick(&["bash"]), None);
        assert_eq!(pick(&["env"]), None);
        assert_eq!(pick(&[]), None);
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
