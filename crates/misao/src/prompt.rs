//! 確認の質問（Node 版の `prompt.ts`）。

use crate::io::CliIo;

/// stderr に質問を出し、stdin から 1 行読む。入力が閉じたら None。
pub async fn ask_line(io: &CliIo, question: &str) -> Option<String> {
    io.stderr.write(question.as_bytes());
    io.stdin.read_line().await
}

/// y / yes なら true。それ以外と、入力が閉じた場合は false。
pub async fn confirm(io: &CliIo, question: &str) -> bool {
    ask_line(io, question).await.is_some_and(|answer| is_yes(&answer))
}

fn is_yes(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

#[cfg(test)]
mod tests {
    use super::is_yes;

    #[test]
    fn only_y_and_yes_confirm() {
        assert!(is_yes("y") && is_yes(" YES ") && is_yes("Yes"));
        assert!(!is_yes("") && !is_yes("n") && !is_yes("yeah"));
    }
}
