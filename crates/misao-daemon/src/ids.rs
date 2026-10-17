use std::sync::{Mutex, PoisonError};

use ulid::{Generator, Ulid};

/// 同一ミリ秒内でも単調増加する ULID を作る（辞書順 = 作成順）。
static GENERATOR: Mutex<Generator> = Mutex::new(Generator::new());

pub fn new_ulid() -> String {
    let mut generator = GENERATOR.lock().unwrap_or_else(PoisonError::into_inner);
    // 同一ミリ秒内に 2^80 個を超えて作ったときだけ失敗する。そのときは新しい乱数で作り直す。
    generator.generate().unwrap_or_else(|_| Ulid::new()).to_string()
}

pub fn new_pane_id() -> String {
    format!("p_{}", new_ulid())
}

pub fn new_window_id() -> String {
    format!("w_{}", new_ulid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_prefix_and_26_char_ulid() {
        let (p, w) = (new_pane_id(), new_window_id());
        assert!(p.starts_with("p_") && p.len() == 28, "{p}");
        assert!(w.starts_with("w_") && w.len() == 28, "{w}");
    }

    #[test]
    fn ulids_increase_monotonically() {
        let ids: Vec<String> = (0..1000).map(|_| new_ulid()).collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
