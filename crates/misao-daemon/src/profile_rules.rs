//! プロファイルの規則（`profiles/*.json`）の型、読み込み時の検証（正規表現のコンパイルなど）、評価。

use regex::Regex;
use serde::Deserialize;

/// 規則が読む範囲の行。小文字にした行と、それをつなげた文字列は、使うときに 1 度だけ作る。
pub(crate) struct RegionLines<'a> {
    lines: &'a [String],
    lowered: &'a [String],
    joined: std::cell::OnceCell<String>,
}

impl<'a> RegionLines<'a> {
    pub(crate) fn new(lines: &'a [String], lowered: &'a [String]) -> Self {
        Self {
            lines,
            lowered,
            joined: std::cell::OnceCell::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn joined(&self) -> &str {
        self.joined.get_or_init(|| self.lowered.join("\n"))
    }

    /// 末尾 `n` 行だけの範囲。
    fn last(&self, n: usize) -> RegionLines<'a> {
        let from = self.lines.len().saturating_sub(n);
        RegionLines::new(&self.lines[from..], &self.lowered[from..])
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuleOutcome {
    Working,
    Blocked,
    Idle,
    Skip,
    Error,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Region {
    Above,
    PromptBox,
    Below,
    All,
    Title,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct LineMatch {
    regex: String,
    #[serde(default)]
    ignore_case: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct LastLines {
    n: usize,
    pub(crate) when: Box<RawCondition>,
}

/// 読み込んだままの条件（正規表現は文字列）。
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RawCondition {
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
pub(crate) struct RawRule {
    pub(crate) id: String,
    pub(crate) verdict: RuleOutcome,
    pub(crate) priority: u32,
    pub(crate) region: Region,
    pub(crate) when: RawCondition,
}

pub(crate) enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    Contains(String),
    Matches(Regex),
    LineMatches(Regex),
    LastLines(usize, Box<Condition>),
}

impl RawCondition {
    pub(crate) fn compile(self) -> Result<Condition, String> {
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
    pub(crate) fn holds(&self, region: &RegionLines<'_>) -> bool {
        match self {
            Self::All(list) => list.iter().all(|c| c.holds(region)),
            Self::Any(list) => list.iter().any(|c| c.holds(region)),
            Self::Not(inner) => !inner.holds(region),
            Self::Contains(needle) => region.joined().contains(needle.as_str()),
            Self::Matches(re) => re.is_match(region.joined()),
            Self::LineMatches(re) => region.lines.iter().any(|line| re.is_match(line)),
            Self::LastLines(n, inner) => inner.holds(&region.last(*n)),
        }
    }
}

pub(crate) struct Rule {
    pub(crate) outcome: RuleOutcome,
    pub(crate) priority: u32,
    pub(crate) region: Region,
    pub(crate) when: Condition,
}
