//! prefix キーの状態機械（Node 版の `attach/prefix-filter.ts`）。

use crate::keys::{ACTIONS, KeyBindings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixAction {
    Detach,
    Next,
    Prev,
    List,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PrefixResult {
    /// ペインへ転送するバイト列。
    pub forward: Vec<u8>,
    /// prefix + アクションキーが押された。以後の入力は処理しない。
    pub action: Option<PrefixAction>,
}

pub struct PrefixFilter {
    keys: KeyBindings,
    is_pending: bool,
}

fn action_of(name: &str) -> PrefixAction {
    match name {
        "detach" => PrefixAction::Detach,
        "next" => PrefixAction::Next,
        "prev" => PrefixAction::Prev,
        _ => PrefixAction::List,
    }
}

impl PrefixFilter {
    pub fn new(keys: KeyBindings) -> Self {
        Self {
            keys,
            is_pending: false,
        }
    }

    /// prefix の次のキーがアクションキーなら action、prefix をもう一度なら prefix の 1 バイトを転送し、
    /// それ以外のキーは捨てる。状態は feed をまたいで保つ。
    pub fn feed(&mut self, data: &[u8]) -> PrefixResult {
        let mut forward = Vec::new();
        for &byte in data {
            if !self.is_pending {
                if byte == self.keys.prefix {
                    self.is_pending = true;
                } else {
                    forward.push(byte);
                }
                continue;
            }
            self.is_pending = false;
            if let Some(name) = ACTIONS.iter().find(|a| self.keys.action_key(a) == Some(byte)) {
                return PrefixResult {
                    forward,
                    action: Some(action_of(name)),
                };
            }
            if byte == self.keys.prefix {
                forward.push(byte);
            }
        }
        PrefixResult {
            forward,
            action: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> PrefixFilter {
        PrefixFilter::new(KeyBindings::default())
    }

    #[test]
    fn plain_input_is_forwarded() {
        let r = filter().feed(b"abc");
        assert_eq!((r.forward, r.action), (b"abc".to_vec(), None));
    }

    #[test]
    fn prefix_then_an_action_key_returns_the_action_and_stops() {
        let r = filter().feed(&[b'a', 0x1e, b'd', b'z']);
        assert_eq!((r.forward, r.action), (b"a".to_vec(), Some(PrefixAction::Detach)));
        for (key, want) in [
            (b'n', PrefixAction::Next),
            (b'p', PrefixAction::Prev),
            (b'l', PrefixAction::List),
        ] {
            assert_eq!(filter().feed(&[0x1e, key]).action, Some(want));
        }
    }

    #[test]
    fn prefix_twice_sends_one_prefix_and_other_keys_are_dropped() {
        let r = filter().feed(&[0x1e, 0x1e, 0x1e, b'x', b'y']);
        assert_eq!((r.forward, r.action), (vec![0x1e, b'y'], None));
    }

    #[test]
    fn the_pending_prefix_survives_across_feeds() {
        let mut f = filter();
        assert_eq!(f.feed(&[0x1e]).action, None);
        assert_eq!(f.feed(b"d").action, Some(PrefixAction::Detach));
    }

    #[test]
    fn custom_keys_are_used() {
        let keys = KeyBindings {
            prefix: 0x01,
            detach: b'q',
            ..KeyBindings::default()
        };
        let mut f = PrefixFilter::new(keys);
        assert_eq!(f.feed(&[0x1e, b'd']).forward, vec![0x1e, b'd']);
        assert_eq!(f.feed(&[0x01, b'q']).action, Some(PrefixAction::Detach));
    }
}
