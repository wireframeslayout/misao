//! ペインの一覧と選択（Node 版の `attach/picker.ts`）。

use crate::io::{CliIo, CliSignal};
use crate::model::PaneInfo;
use crate::prompt::ask_line;
use crate::view::pane_view::{
    display_name, foreground_command, pane_state_key, relative_time, short_display_name, sort_panes,
    state_label, task_label,
};
use crate::view::table::render_table;

/// Signal は入力待ちの間に終了シグナルを受けた。
#[derive(Debug, Clone, PartialEq)]
pub enum PickerChoice {
    Pane(Box<PaneInfo>),
    New,
    Quit,
    Signal,
}

/// attach できるペイン（終了済み・停止中を除く）。ls と同じ並び。
pub fn attachable_panes(panes: &[PaneInfo]) -> Vec<PaneInfo> {
    sort_panes(panes)
        .into_iter()
        .filter(|p| !matches!(pane_state_key(p), "exited" | "stopped"))
        .collect()
}

/// 一覧で blocked だったペインが、入る時点では working になっていれば知らせる文言。
pub fn resumed_notice(listed: &PaneInfo, now: &PaneInfo, home_dir: &str) -> Option<String> {
    (listed.agent_state == "blocked" && now.agent_state == "working").then(|| {
        format!(
            "{} はいま再開しました（blocked → working）",
            short_display_name(now, home_dir)
        )
    })
}

pub struct PickerOptions<'a> {
    /// 一覧の前に出す行（抜けたときの案内など）。
    pub header: &'a [String],
    /// n（新しいペイン）を受け付けるか。--readonly では false。
    pub can_create: bool,
}

/// 番号付きの一覧を出し、番号 / n / q を受け付ける。入力が閉じたら Quit、終了シグナルを受けたら Signal。
pub async fn run_picker(io: &CliIo, panes: &[PaneInfo], options: PickerOptions<'_>) -> PickerChoice {
    let choices = attachable_panes(panes);
    for line in options.header {
        io.out_line(line);
    }
    if choices.is_empty() {
        io.out_line("[misao] 入れるペインがありません");
    } else {
        let mut rows: Vec<Vec<String>> = vec![
            ["#", "STATE", "NAME", "TASK", "AGENT", "LAST"]
                .map(String::from)
                .to_vec(),
        ];
        rows.extend(choices.iter().enumerate().map(|(i, p)| {
            vec![
                (i + 1).to_string(),
                state_label(p),
                display_name(p, &io.home_dir),
                task_label(p),
                foreground_command(p),
                relative_time(p.last_output_at.as_deref(), io.now_ms()),
            ]
        }));
        for line in render_table(&rows) {
            io.out_line(&format!("  {line}"));
        }
    }
    let mut guide = vec!["番号で入る"];
    if options.can_create {
        guide.push("n で新しいペイン");
    }
    guide.push("q で終了");
    let guide = guide.join(" · ");
    io.out_line(&guide);
    let mut signals = io.signals.listen(&CliSignal::TERMINATION);
    loop {
        let answer = tokio::select! {
            answer = ask_line(io, "> ") => answer,
            _ = signals.recv() => return PickerChoice::Signal,
        };
        let Some(answer) = answer else {
            return PickerChoice::Quit;
        };
        let answer = answer.trim().to_lowercase();
        if answer == "q" {
            return PickerChoice::Quit;
        }
        if answer == "n" && options.can_create {
            return PickerChoice::New;
        }
        let chosen = if !answer.is_empty() && answer.bytes().all(|b| b.is_ascii_digit()) {
            answer
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| choices.get(i))
        } else {
            None
        };
        if let Some(pane) = chosen {
            return PickerChoice::Pane(Box::new(pane.clone()));
        }
        io.err_line(&format!("[misao] {guide} のいずれかを入力してください"));
    }
}
