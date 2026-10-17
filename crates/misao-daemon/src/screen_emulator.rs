//! 端末の状態と、文字・制御列の意味（xterm.js の `InputHandler` のうち、画面の文字に関わる部分）。
//! 色・属性、マウス、応答の送信などは扱わない。意味は `@xterm/headless` 6 に合わせる（差分テストで確かめている）。

use crate::screen_attr::Attr;
use crate::screen_buffer::Buffer;
use crate::screen_charset::Charset;
use crate::unicode_width::{CharProps, char_properties};

/// 画面の大きさの下限（xterm.js と同じ: 2 列 x 1 行）。
pub const MIN_COLS: usize = 2;
pub const MIN_ROWS: usize = 1;

/// マウスの報告の種類（`CSI ? 9 / 1000 / 1002 / 1003 h`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
    None,
    X10,
    Vt200,
    Drag,
    Any,
}

/// 端末の入力に関わるモード。画面の文字には効かないが、attach の snapshot が復元する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputModes {
    pub app_cursor_keys: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub send_focus: bool,
    pub mouse: MouseMode,
}

impl InputModes {
    const DEFAULT: Self = Self {
        app_cursor_keys: false,
        app_keypad: false,
        bracketed_paste: false,
        send_focus: false,
        mouse: MouseMode::None,
    };
}

pub struct Emulator {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    pub(crate) scrollback: usize,
    pub(crate) normal: Buffer,
    pub(crate) alt: Buffer,
    pub(crate) alt_active: bool,
    pub(crate) insert_mode: bool,
    pub(crate) convert_eol: bool,
    pub(crate) origin: bool,
    pub(crate) wraparound: bool,
    pub(crate) reverse_wraparound: bool,
    /// 現在の文字属性（`CSI m` で設定し、書いたセルに付く）。
    pub(crate) attr: Attr,
    pub(crate) input: InputModes,
    pub(crate) gsets: [Option<Charset>; 4],
    pub(crate) glevel: usize,
    pub(crate) charset: Option<Charset>,
    pub(crate) preceding: CharProps,
    /// 連続して届いた表示文字（`print` はこの単位で処理する。xterm.js のパーサーが呼ぶ単位と同じ）。
    pub(crate) run: Vec<u32>,
    pub titles: Vec<String>,
    /// 中止させる 1 バイトを渡している間だけ true。中止される OSC の dispatch を捨てる。
    pub(crate) discard_osc: bool,
}

impl Emulator {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let (normal, alt) = Self::fresh_buffers(cols, rows, scrollback);
        Self {
            cols,
            rows,
            scrollback,
            normal,
            alt,
            alt_active: false,
            insert_mode: false,
            convert_eol: false,
            origin: false,
            wraparound: true,
            reverse_wraparound: false,
            attr: Attr::DEFAULT,
            input: InputModes::DEFAULT,
            gsets: [None; 4],
            glevel: 0,
            charset: None,
            preceding: CharProps::default(),
            run: Vec::new(),
            titles: Vec::new(),
            discard_osc: false,
        }
    }

    fn fresh_buffers(cols: usize, rows: usize, scrollback: usize) -> (Buffer, Buffer) {
        let mut normal = Buffer::new(cols, rows, scrollback, true);
        normal.fill_viewport_rows(Attr::DEFAULT);
        (normal, Buffer::new(cols, rows, scrollback, false))
    }

    pub fn buf(&mut self) -> &mut Buffer {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.normal
        }
    }

    pub fn buf_ref(&self) -> &Buffer {
        if self.alt_active { &self.alt } else { &self.normal }
    }

    pub fn is_alt(&self) -> bool {
        self.alt_active
    }

    // ---- 表示文字 ----

    pub fn push_printable(&mut self, c: char) {
        self.run.push(c as u32);
    }

    /// 溜めた表示文字を処理する。表示文字以外の制御列が来る前と、1 回の出力の終わりに呼ぶ。
    pub fn flush_run(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let mut run = std::mem::take(&mut self.run);
        self.print(&run);
        // 確保した領域を次の連続のために使い回す。
        run.clear();
        self.run = run;
    }

    pub(crate) fn print(&mut self, data: &[u32]) {
        if data.is_empty() {
            return;
        }
        let (cols, rows) = (self.cols, self.rows);
        let (wraparound, insert_mode) = (self.wraparound, self.insert_mode);
        let (charset, attr, erase) = (self.charset, self.attr, self.attr.erase());
        let mut preceding = self.preceding;
        let b = self.buf();
        if b.x > 0 && b.row(b.y).width(b.x - 1) == 2 {
            let (x, y) = (b.x, b.y);
            b.row_mut(y).set_null(x - 1, attr);
        }
        for &raw in data {
            let code = charset.map_or(raw, |cs| cs.translate(raw));
            let info = char_properties(code, preceding);
            let width = usize::from(info.width());
            let old_width = if info.should_join() {
                usize::from(preceding.width())
            } else {
                0
            };
            preceding = info;
            if b.x + width > cols + old_width {
                if wraparound {
                    Self::wrap_line(b, rows, cols, old_width, attr, erase);
                } else {
                    b.x = cols - 1;
                    if width == 2 {
                        continue;
                    }
                }
            }
            let (x, y) = (b.x, b.y);
            if info.should_join() && x > 0 {
                let row = b.row_mut(y);
                let offset = if row.width(x - 1) == 0 { 2 } else { 1 };
                row.add_code_to_cell(x - offset, code, info.width());
                for i in 0..width - old_width {
                    row.set_code(x + i, 0, 0, attr);
                }
                b.x += width - old_width;
                continue;
            }
            let row = b.row_mut(y);
            if insert_mode {
                row.insert_cells(x, width - old_width, attr);
                if row.width(cols - 1) == 2 {
                    row.set_code(cols - 1, 0, 1, attr);
                }
            }
            row.set_code(x, code, info.width(), attr);
            for i in 1..width {
                row.set_code(x + i, 0, 0, attr);
            }
            b.x += width.max(1);
        }
        self.preceding = preceding;
        let b = self.buf();
        let (x, y) = (b.x, b.y);
        if x < cols && b.row(y).width(x) == 0 && !b.row(y).has_content(x) {
            b.row_mut(y).set_code(x, 0, 1, attr);
        }
    }

    /// 行末で折り返す（`print` の自動折り返し）。結合文字で幅が広がったときは、直前の文字を次の行へ移す。
    fn wrap_line(b: &mut Buffer, rows: usize, cols: usize, old_width: usize, attr: Attr, erase: Attr) {
        let mut old_col = b.x - old_width;
        let old_y = b.y;
        let carried: Vec<_> = (0..old_width)
            .map(|i| b.row(old_y).cell(old_col + i).clone())
            .collect();
        while old_col < cols {
            b.row_mut(old_y).set_null(old_col, attr);
            old_col += 1;
        }
        b.x = old_width;
        b.y += 1;
        if b.y == b.scroll_bottom + 1 {
            b.y -= 1;
            b.scroll(true, erase);
        } else {
            if b.y >= rows {
                b.y = rows - 1;
            }
            let y = b.y;
            b.row_mut(y).is_wrapped = true;
        }
        let y = b.y;
        for (i, cell) in carried.into_iter().enumerate() {
            b.row_mut(y).set_cell(i, cell);
        }
    }

    // ---- カーソルの保存・モード ----

    pub fn save_cursor(&mut self) {
        let (charset, attr) = (self.charset, self.attr);
        let b = self.buf();
        b.saved.x = b.x;
        b.saved.y = b.ybase + b.y;
        b.saved.charset = charset;
        b.saved.attr = attr;
    }

    pub fn restore_cursor(&mut self) {
        let b = self.buf();
        b.x = b.saved.x;
        b.y = b.saved.y.saturating_sub(b.ybase);
        let (charset, attr) = (b.saved.charset, b.saved.attr);
        self.charset = charset;
        self.attr = attr;
        self.restrict_cursor(None);
    }

    pub fn set_insert_mode(&mut self, on: bool) {
        self.insert_mode = on;
    }

    pub fn set_convert_eol(&mut self, on: bool) {
        self.convert_eol = on;
    }

    pub fn set_origin_mode(&mut self, on: bool) {
        self.origin = on;
        self.set_cursor(0, 0);
    }

    pub fn set_wraparound(&mut self, on: bool) {
        self.wraparound = on;
    }

    pub fn set_reverse_wraparound(&mut self, on: bool) {
        self.reverse_wraparound = on;
    }

    pub fn activate_alt(&mut self) {
        if !self.alt_active {
            // 代替画面の行は、消去と同じ属性（背景色）で作る（xterm.js の `activateAltBuffer(eraseAttr)`）。
            let erase = self.attr.erase();
            self.alt.fill_viewport_rows(erase);
            self.alt.x = self.normal.x;
            self.alt.y = self.normal.y;
            self.alt_active = true;
        }
    }

    pub fn activate_normal(&mut self) {
        if self.alt_active {
            self.normal.x = self.alt.x;
            self.normal.y = self.alt.y;
            self.alt.clear();
            self.alt_active = false;
        }
    }

    // ---- 文字集合 ----

    pub fn set_glevel(&mut self, level: usize) {
        self.glevel = level;
        self.charset = self.gsets[level];
    }

    pub fn set_gcharset(&mut self, index: usize, charset: Option<Charset>) {
        self.gsets[index] = charset;
        if self.glevel == index {
            self.charset = charset;
        }
    }

    pub fn reset_charsets(&mut self) {
        for index in 0..4 {
            self.set_gcharset(index, None);
        }
    }

    // ---- リセット ----

    fn reset_modes(&mut self) {
        self.insert_mode = false;
        self.origin = false;
        self.wraparound = true;
        self.reverse_wraparound = false;
        self.input.app_cursor_keys = false;
        self.input.app_keypad = false;
        self.input.bracketed_paste = false;
        self.input.send_focus = false;
    }

    /// `CSI ! p`
    pub fn soft_reset(&mut self) {
        let rows = self.rows;
        self.reset_modes();
        self.gsets = [None; 4];
        self.glevel = 0;
        self.charset = None;
        let b = self.buf();
        b.scroll_top = 0;
        b.scroll_bottom = rows - 1;
        b.saved.x = 0;
        b.saved.y = b.ybase;
        b.saved.charset = None;
        b.saved.attr = Attr::DEFAULT;
        self.attr = Attr::DEFAULT;
    }

    /// `ESC c`
    pub fn full_reset(&mut self) {
        let (normal, alt) = Self::fresh_buffers(self.cols, self.rows, self.scrollback);
        self.normal = normal;
        self.alt = alt;
        self.alt_active = false;
        self.reset_modes();
        self.input.mouse = MouseMode::None;
        self.attr = Attr::DEFAULT;
        self.gsets = [None; 4];
        self.glevel = 0;
        self.charset = None;
        self.preceding = CharProps::default();
    }

    pub fn reset_preceding(&mut self) {
        self.preceding = CharProps::default();
    }

    /// 画面の大きさを変える（通常画面と代替画面の両方）。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.normal.resize(cols, rows);
        self.alt.resize(cols, rows);
        self.normal.setup_tab_stops(Some(cols));
        self.alt.setup_tab_stops(Some(cols));
    }
}
