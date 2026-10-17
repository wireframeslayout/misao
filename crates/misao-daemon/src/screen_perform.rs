//! `vte` のパーサーが見つけた制御列を、`Emulator` の操作へ振り分ける（xterm.js が `InputHandler` に登録するハンドラの表）。
//! ここに無い列（色・属性、マウス、応答を返す問い合わせなど）は無視する。

use vte::{Params, Perform};

use crate::screen_charset::{Charset, is_charset_final};
use crate::screen_emulator::{Emulator, MouseMode};

/// i 番目のパラメータ（無ければ 0）。
fn param(params: &Params, i: usize) -> usize {
    params
        .iter()
        .nth(i)
        .and_then(|group| group.first())
        .map_or(0, |&v| usize::from(v))
}

/// `params[i] || 1`
fn count(params: &Params, i: usize) -> usize {
    match param(params, i) {
        0 => 1,
        n => n,
    }
}

fn each_param(params: &Params) -> impl Iterator<Item = usize> + '_ {
    params
        .iter()
        .map(|group| group.first().map_or(0, |&v| usize::from(v)))
}

impl Emulator {
    fn set_modes(&mut self, params: &Params, on: bool) {
        for mode in each_param(params) {
            match mode {
                4 => self.set_insert_mode(on),
                20 => self.set_convert_eol(on),
                _ => {}
            }
        }
    }

    fn set_private_modes(&mut self, params: &Params, on: bool) {
        for mode in each_param(params) {
            match (mode, on) {
                (1, _) => self.input.app_cursor_keys = on,
                (66, _) => self.input.app_keypad = on,
                (9, _) => self.set_mouse(MouseMode::X10, on),
                (1000, _) => self.set_mouse(MouseMode::Vt200, on),
                (1002, _) => self.set_mouse(MouseMode::Drag, on),
                (1003, _) => self.set_mouse(MouseMode::Any, on),
                (1004, _) => self.input.send_focus = on,
                (2004, _) => self.input.bracketed_paste = on,
                (2, true) => self.reset_charsets(),
                (6, _) => self.set_origin_mode(on),
                (7, _) => self.set_wraparound(on),
                (45, _) => self.set_reverse_wraparound(on),
                (1048, true) => self.save_cursor(),
                (1048, false) => self.restore_cursor(),
                (1049, true) => {
                    self.save_cursor();
                    self.activate_alt();
                }
                (47 | 1047, true) => self.activate_alt(),
                (1049, false) => {
                    self.activate_normal();
                    self.restore_cursor();
                }
                (47 | 1047, false) => self.activate_normal(),
                _ => {}
            }
        }
    }

    /// マウスの報告は最後に設定したものが有効。どれかを解除すると報告なしになる（xterm.js と同じ）。
    fn set_mouse(&mut self, mode: MouseMode, on: bool) {
        self.input.mouse = if on { mode } else { MouseMode::None };
    }

    fn select_charset(&mut self, intermediate: u8, final_byte: u8) {
        let target = match intermediate {
            b'(' => 0,
            b')' | b'-' => 1,
            b'*' | b'.' => 2,
            b'+' => 3,
            _ => return,
        };
        self.set_gcharset(target, Charset::from_final(char::from(final_byte)));
    }

    fn set_title_from_osc(&mut self, params: &[&[u8]]) {
        let title = params[1..]
            .iter()
            .map(|part| String::from_utf8_lossy(part))
            .collect::<Vec<_>>()
            .join(";");
        self.titles.push(title);
    }
}

impl Perform for Emulator {
    fn print(&mut self, c: char) {
        self.push_printable(c);
    }

    fn execute(&mut self, byte: u8) {
        self.flush_run();
        match byte {
            0x08 => self.backspace(),
            0x09 => self.tab(),
            0x0A..=0x0C => self.line_feed(),
            0x0D => self.carriage_return(),
            0x0E => self.set_glevel(1),
            0x0F => self.set_glevel(0),
            _ => {}
        }
        self.reset_preceding();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        self.flush_run();
        // 中止された OSC（CAN / SUB、長さの上限）は何も起こさない。
        if self.discard_osc {
            self.reset_preceding();
            return;
        }
        match params.first() {
            Some(&b"0" | &b"2") if params.len() >= 2 => self.set_title_from_osc(params),
            Some(&b"0" | &b"2") => self.titles.push(String::new()),
            _ => {}
        }
        self.reset_preceding();
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.flush_run();
        if !ignore {
            self.dispatch_csi(params, intermediates, action);
        }
        // 繰り返し（REP）が見るのは、直前の表示文字だけ。制御列を挟んだら忘れる。
        self.reset_preceding();
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.flush_run();
        if !ignore {
            self.dispatch_esc(intermediates, byte);
        }
        self.reset_preceding();
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        self.flush_run();
    }

    fn unhook(&mut self) {
        self.reset_preceding();
    }
}

impl Emulator {
    fn dispatch_csi(&mut self, params: &Params, intermediates: &[u8], action: char) {
        match (intermediates, action) {
            ([], '@') => self.insert_chars(count(params, 0)),
            ([], 'A') => self.cursor_up(count(params, 0)),
            ([], 'B') => self.cursor_down(count(params, 0)),
            ([], 'C' | 'a') => self.cursor_forward(count(params, 0)),
            ([], 'D') => self.cursor_backward(count(params, 0)),
            ([], 'E') => self.cursor_next_line(count(params, 0)),
            ([], 'F') => self.cursor_preceding_line(count(params, 0)),
            ([], 'G' | '`') => self.cursor_column(count(params, 0)),
            ([], 'H' | 'f') => {
                let col = if params.len() >= 2 { count(params, 1) } else { 1 };
                self.cursor_position(count(params, 0), col);
            }
            ([], 'I') => self.cursor_forward_tab(count(params, 0)),
            ([] | [b'?'], 'J') => self.erase_in_display(param(params, 0)),
            ([] | [b'?'], 'K') => self.erase_in_line(param(params, 0)),
            ([], 'L') => self.insert_lines(count(params, 0)),
            ([], 'M') => self.delete_lines(count(params, 0)),
            ([], 'P') => self.delete_chars(count(params, 0)),
            ([], 'S') => self.scroll_up(count(params, 0)),
            ([], 'T') => self.scroll_down(count(params, 0)),
            ([], 'X') => self.erase_chars(count(params, 0)),
            ([], 'Z') => self.cursor_backward_tab(count(params, 0)),
            ([], 'b') => self.repeat_preceding(count(params, 0)),
            ([], 'd') => self.cursor_line(count(params, 0)),
            ([], 'e') => self.vertical_relative(count(params, 0)),
            ([], 'g') => self.tab_clear(param(params, 0)),
            ([], 'h') => self.set_modes(params, true),
            ([], 'l') => self.set_modes(params, false),
            ([], 'm') => self.attr.apply_sgr(params),
            ([b'?'], 'h') => self.set_private_modes(params, true),
            ([b'?'], 'l') => self.set_private_modes(params, false),
            ([], 'r') => {
                let bottom = (params.len() >= 2).then(|| param(params, 1));
                self.set_scroll_region(count(params, 0), bottom);
            }
            ([], 's') => self.save_cursor(),
            ([], 'u') => self.restore_cursor(),
            ([b'!'], 'p') => self.soft_reset(),
            _ => {}
        }
    }

    fn dispatch_esc(&mut self, intermediates: &[u8], byte: u8) {
        match (intermediates, char::from(byte)) {
            ([], '7') => self.save_cursor(),
            ([], '8') => self.restore_cursor(),
            ([], 'D') => self.index(),
            ([], 'E') => self.next_line(),
            ([], 'H') => self.tab_set(),
            ([], 'M') => self.reverse_index(),
            ([], 'c') => self.full_reset(),
            ([], '=') => self.input.app_keypad = true,
            ([], '>') => self.input.app_keypad = false,
            ([], 'n') | ([], '}') => self.set_glevel(2),
            ([], 'o') | ([], '|') => self.set_glevel(3),
            ([], '~') => self.set_glevel(1),
            ([b'%'], '@' | 'G') => {
                self.set_glevel(0);
                self.set_gcharset(0, None);
            }
            ([b'#'], '8') => self.screen_alignment_pattern(),
            ([intermediate @ (b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/')], f) if is_charset_final(f) => {
                self.select_charset(*intermediate, byte);
            }
            _ => {}
        }
    }
}
