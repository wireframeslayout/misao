//! `attach`（端末をペインにつなぐ）。prefix キーの状態機械、端末の raw mode、一覧（picker）、1 ペインのセッション、ペイン間の移動。

pub mod loop_;
pub mod picker;
pub mod prefix_filter;
pub mod session;
pub mod tty;

#[cfg(all(test, unix))]
mod fake_daemon;

#[cfg(all(test, unix))]
mod tests;
