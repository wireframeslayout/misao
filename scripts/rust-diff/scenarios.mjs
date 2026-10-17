// Rust 版との差分テスト用の入力列。各シナリオは「ある時刻 (pane 起動からのミリ秒) に何が起きたか」の列で、
// 出力 (output)・入力 (input)・resize・画面の読み出し (screen)・稼働判定の読み出し (info)・子の終了 (exit) から成る。
// 期待値は gen-rust-diff-fixtures.mjs が Node 版 (xterm + StateTracker) で作る。ここは入力だけ。

const ESC = '\x1b';
const CSI = `${ESC}[`;
const sgr = (...p) => `${CSI}${p.join(';')}m`;
const RESET = sgr(0);

/** 先頭が at の時刻に起こす操作を作る小さなビルダー。 */
const out = (at, data) => ({ at, op: 'output', ...(typeof data === 'string' ? { data } : { bytes: [...data] }) });
const input = (at) => ({ at, op: 'input' });
const resize = (at, cols, rows) => ({ at, op: 'resize', cols, rows });
const screen = (at) => ({ at, op: 'screen' });
const info = (at) => ({ at, op: 'info' });
const exit = (at) => ({ at, op: 'exit' });

const title = (t) => `${ESC}]0;${t}\x07`;
const rule = (cols) => '─'.repeat(cols);
const at = (row, col) => `${CSI}${row};${col}H`;

/** Ink (Claude Code の UI ライブラリ) 風の再描画: 直前のフレームの行数ぶん消してから新しいフレームを書く。 */
function inkFrame(prevLines, lines) {
  let erase = '';
  for (let i = 0; i < prevLines; i++) erase += `${CSI}2K${i < prevLines - 1 ? `${CSI}1A` : ''}`;
  return `${erase}${CSI}G${lines.join('\n')}\n`;
}

function claudeFrame(cols, { above = [], box = ['❯ '], below = ['  ? for shortcuts'] }) {
  return [...above, rule(cols), ...box, rule(cols), ...below];
}

const WELCOME = [
  `${sgr(38, 5, 174)}╭─── Claude Code v2.1.263 ──────────────────────────────────────────────────────╮${RESET}`,
  `${sgr(38, 5, 174)}│${RESET}  ${sgr(1)}Welcome back!${RESET}                                                              ${sgr(38, 5, 174)}│${RESET}`,
  `${sgr(38, 5, 174)}│${RESET}  ~/workspace/misao                                                           ${sgr(38, 5, 174)}│${RESET}`,
  `${sgr(38, 5, 174)}╰───────────────────────────────────────────────────────────────────────────────╯${RESET}`,
];

function claudeSession() {
  const cols = 100;
  const steps = [];
  let t = 0;
  let prev = 0;
  const frame = (dt, lines) => {
    t += dt;
    steps.push(out(t, inkFrame(prev, lines)));
    prev = lines.length;
  };
  steps.push(out(t, `${CSI}?2026h${CSI}?25l${title('✳ Claude Code')}`));
  frame(300, [...WELCOME, '', ...claudeFrame(cols, {})]);
  steps.push(out(t, `${CSI}?25h`), info(t + 10), screen(t + 20));
  // 入力 (エコー) → 作業中。スピナーの行とタイトルのスピナーが 1 秒ごとに進む。
  t += 2000;
  steps.push(input(t));
  frame(40, [...WELCOME, '', `> list the files and run the tests`, '', ...claudeFrame(cols, { box: ['❯ '] })]);
  const glyphs = ['⠂', '⠐', '⠒', '⠲'];
  const spin = ['✻', '✶', '✽', '✢'];
  for (let i = 0; i < 9; i++) {
    t += 1000;
    steps.push(out(t, title(`${glyphs[i % 4]} Claude Code`)));
    frame(5, [
      `> list the files and run the tests`,
      '',
      `${sgr(38, 5, 174)}${spin[i % 4]}${RESET} Pondering… (${i + 1}s · ↑ ${(i + 1) * 120} tokens · esc to interrupt)`,
      '',
      ...claudeFrame(cols, { below: ['  ⏵⏵ accept edits on (shift+tab to cycle) · esc to interrupt'] }),
    ]);
    if (i === 4) steps.push(info(t + 10), screen(t + 20));
  }
  // 許可プロンプト (blocked)
  t += 700;
  steps.push(out(t, title('✳ Claude Code')));
  frame(10, [
    `${sgr(1)}⏺ Bash${RESET}(rm -rf build)`,
    '',
    ...claudeFrame(cols, {
      box: [' Bash command', '', '   rm -rf build', '', ' Do you want to proceed?', ' ❯ 1. Yes', '   2. Yes, and don\'t ask again for rm commands', '   3. No, and tell Claude what to do differently (esc)'],
      below: [' Esc to cancel · Tab to add additional instructions'],
    }),
  ]);
  steps.push(info(t + 200), screen(t + 210));
  // 承認 → 少し作業 → 完了 (idle)
  t += 3000;
  steps.push(input(t));
  frame(30, ['⏺ Bash(rm -rf build)', '  ⎿  (No content)', '', ...claudeFrame(cols, {})]);
  steps.push(info(t + 100));
  t += 7000;
  steps.push(info(t), screen(t + 5), exit(t + 100));
  return { name: 'claude-session', cmd: ['claude'], cols, rows: 30, steps, end: t + 3000 };
}

function claudeQuestion() {
  const cols = 90;
  const steps = [out(0, `${CSI}?2026h${title('✳ Claude Code')}`)];
  let t = 400;
  steps.push(out(t, inkFrame(0, claudeFrame(cols, { above: ['⏺ Which approach do you prefer?'], box: ['❯ '] }))));
  t += 5000;
  steps.push(info(t));
  steps.push(
    out(
      t,
      inkFrame(
        4,
        claudeFrame(cols, {
          above: ['⏺ Which approach do you prefer?'],
          box: [' ☐ Approach', '', ' Which approach?', ' ❯ 1. Fast path', '   2. Careful path'],
          below: [' Enter to select · ↑/↓ to navigate · Esc to cancel'],
        }),
      ),
    ),
  );
  steps.push(info(t + 300), screen(t + 310));
  t += 6000;
  // transcript viewer (skip) は blocked の文言が残っていても意見を持たない
  steps.push(out(t, `${CSI}?1049h${CSI}2J${CSI}H Transcript\r\n Do you want to proceed?\r\n${at(30, 1)}Showing detailed transcript · ctrl+o to toggle`));
  steps.push(info(t + 200), screen(t + 210));
  t += 4000;
  steps.push(out(t, `${CSI}?1049l`), info(t + 200), screen(t + 210));
  return { name: 'claude-question-transcript', cmd: ['env', '-u', 'CLAUDE_CODE', 'claude', '--model', 'x'], cols, rows: 30, steps, end: t + 5000 };
}

function claudeApiError() {
  const cols = 80;
  const steps = [out(0, title('✳ Claude Code'))];
  steps.push(out(200, inkFrame(0, claudeFrame(cols, { above: ['⏺ working on it', '  ⎿  API Error: Request timed out.'], box: ['❯ '] }))));
  steps.push(info(500), screen(510));
  steps.push(out(1000, inkFrame(5, claudeFrame(cols, { above: ['⏺ working on it', '  ⎿  API Error: Request timed out. Retrying in 4 seconds…'], box: ['❯ '] }))));
  steps.push(info(1300));
  return { name: 'claude-api-error', cmd: ['claude'], cols, rows: 24, steps, end: 8000 };
}

function shellWrappedClaude() {
  // シェルへ文字列で渡した claude も、claude の profile で判定する
  const cols = 80;
  const steps = [out(0, title('✳ Claude Code')), out(200, inkFrame(0, claudeFrame(cols, { above: ['⏺ Do you want to proceed?'], box: [' ❯ 1. Yes', '   2. No'] })))];
  steps.push(info(500));
  return { name: 'claude-via-shell', cmd: ['/bin/zsh', '-lc', 'cd /w && FOO=1 exec \'claude\' --x'], cols, rows: 24, steps, end: 3000 };
}

function codexSession() {
  const steps = [out(0, title('codex'))];
  steps.push(out(300, ` OpenAI Codex\r\n\r\n ▌ Ask Codex to do anything\r\n`));
  steps.push(info(400), screen(410));
  steps.push(input(1500));
  for (let i = 0; i < 6; i++) {
    steps.push(out(2000 + i * 900, `${CSI}2K\r  Working (${i}s • esc to interrupt) ${'.'.repeat(i)}\r\n${'x'.repeat(180)}\r\n`));
  }
  steps.push(info(7200));
  steps.push(out(8000, title('Action Required - codex')));
  steps.push(info(8200));
  steps.push(out(8300, title('codex')));
  steps.push(out(8400, `\r\n Allow command? [y/n]\r\n`));
  steps.push(info(8700), screen(8710));
  steps.push(input(9500));
  steps.push(out(9600, `${CSI}2J${CSI}H ▌ Ask Codex to do anything\r\n`));
  steps.push(info(9900), screen(9910));
  return { name: 'codex-session', cmd: ['/usr/local/bin/codex'], cols: 100, rows: 30, steps, end: 18000 };
}

function bytesOnly() {
  // profile なし。出力量だけで working / idle になる。入力と resize の猶予も確かめる。
  const steps = [out(0, '$ ')];
  const chunk = (n) => 'abcdefghij'.repeat(n / 10) + '\r\n';
  for (let i = 0; i < 6; i++) steps.push(out(1100 + i * 700, chunk(160)));
  steps.push(info(5600));
  steps.push(input(9000), out(9100, chunk(400))); // 入力直後の出力は数えない
  steps.push(out(9700, chunk(400)));
  steps.push(resize(11000, 120, 30), out(11200, chunk(600))); // resize 直後の出力は数えない
  steps.push(out(12000, chunk(60)), out(12700, chunk(60)), out(13400, chunk(60)));
  steps.push(info(14000), screen(14010));
  // 1 つのティックだけ閾値を超えても working にはならない
  steps.push(out(21500, chunk(300)));
  steps.push(info(22500), info(30000));
  return { name: 'bytes-only', cmd: ['bash'], cols: 80, rows: 24, steps, end: 40000 };
}

function titleOnly() {
  const steps = [out(0, title('~ shell'))];
  steps.push(out(1000, title('⠋ build')), info(1100));
  steps.push(out(2200, title('⠙ build')), info(2300));
  steps.push(info(5500)); // 3 秒更新が無いスピナーは意見なし
  steps.push(out(6000, title('done')), info(6100)); // スピナーを見た後の通常のタイトルは idle
  steps.push(out(7000, title('')), info(7100));
  steps.push(out(7500, title('◐ spin')), out(7600, title('plain')), info(7700));
  steps.push(out(8000, `${ESC}]2;two;parts\x07`), screen(8010));
  steps.push(out(9000, `${ESC}]0;${'long'.repeat(40)}\x1b\\`), screen(9010));
  return { name: 'title-only', cmd: ['bash'], cols: 80, rows: 24, steps, end: 20000 };
}

function exitFlow() {
  const steps = [out(0, 'start\r\n')];
  for (let i = 0; i < 5; i++) steps.push(out(1100 + i * 600, `${'z'.repeat(220)}\r\n`));
  steps.push(info(4200));
  steps.push(out(4400, 'bye'), exit(4500), screen(4600), info(4700));
  return { name: 'exit-flow', cmd: ['sh'], cols: 80, rows: 24, steps, end: 12000 };
}

function plainTerminal() {
  const lines = [];
  for (let i = 0; i < 60; i++) lines.push(`line ${String(i).padStart(2, '0')} ${sgr(32)}green${RESET} ${'=>'.repeat(i % 20)}`);
  const steps = [
    out(0, `user@host:~$ ls --color\r\n${lines.slice(0, 30).join('\r\n')}\r\n`),
    screen(100),
    out(200, `${lines.slice(30).join('\r\n')}\r\nuser@host:~$ `),
    screen(300),
    out(400, 'tab\there\tand more\tstuff\r\nback\b\bsp\r\nover\rXY\r\n'),
    out(500, `${'W'.repeat(80)}${'X'.repeat(80)}${'Y'.repeat(7)}\r\n`),
    out(600, `${'W'.repeat(79)}\r\n${'V'.repeat(80)}`),
    screen(700),
    out(800, `\r\n${CSI}1;31mred bold${RESET} ${CSI}7minverse${RESET} ${CSI}4munder${RESET} ${CSI}38;2;10;20;30mtruecolor${RESET}\r\n`),
    screen(900),
  ];
  return { name: 'plain-terminal', cmd: ['bash'], cols: 80, rows: 24, steps, end: 2000 };
}

function wideChars() {
  const steps = [
    out(0, '日本語のテキスト、ＡＢＣ　全角スペース。\r\n'),
    out(10, `${'あ'.repeat(40)}い\r\n`),
    out(20, `${'a'.repeat(79)}あう\r\n`),
    out(30, 'é ä ñ combining\r\n'),
    out(40, '┌────────┐\r\n│ 箱 罫線 │\r\n└────────┘\r\n'),
    out(50, '❯ ⏺ ✻ ✳ ⎿ · … → ← ↑ ↓ ▐▛███▜▌\r\n'),
    // UTF-8 の 1 文字を 2 つのチャンクに割る
    out(60, new Uint8Array([0xe6, 0x97])),
    out(61, new Uint8Array([0xa5, 0x0d, 0x0a])),
    screen(100),
  ];
  return { name: 'wide-chars', cmd: ['bash'], cols: 80, rows: 24, steps, end: 1000 };
}

function altScreen() {
  const steps = [
    out(0, 'main screen line 1\r\nmain screen line 2\r\n'),
    out(100, `${CSI}?1049h${CSI}2J${CSI}H${title('vim demo')}`),
    out(110, `${CSI}1;1Hheader${CSI}3;5Hindented${CSI}5;1H${CSI}K~${CSI}24;1H-- INSERT --${CSI}2;1H`),
    screen(200),
    out(300, `${CSI}2;23r${CSI}23;1H\r\nscrolled-in line\r\n\r\n${CSI}r`),
    screen(400),
    out(500, `${CSI}1;1H${CSI}2L${CSI}1;1Hinserted${CSI}M${CSI}2P${CSI}4@`),
    screen(600),
    out(700, `${CSI}?1049l`),
    screen(800),
    out(900, `${ESC}7saved${ESC}8${CSI}2Aup${CSI}3Cright${CSI}10Gabs${CSI}5dline5`),
    screen(1000),
  ];
  return { name: 'alt-screen', cmd: ['vim'], cols: 80, rows: 24, steps, end: 2000 };
}

function resizeFlow() {
  const steps = [
    out(0, 'one\r\ntwo two two two two two two two two two two two two two\r\nthree\r\n'),
    screen(50),
    resize(100, 120, 30),
    screen(150),
    out(200, 'after-resize\r\n'),
    resize(300, 40, 10),
    screen(350),
    out(400, 'tail-line\r\n'),
    screen(450),
    resize(500, 80, 24),
    screen(550),
  ];
  return { name: 'resize-flow', cmd: ['bash'], cols: 80, rows: 24, steps, end: 1000 };
}

function altScreenResize() {
  // 代替画面を使っている間の resize (使う前に resize しない: 空の代替画面を resize してから使うと xterm.js の内部状態が食い違う)
  const steps = [
    out(0, 'main line\r\n'),
    out(10, `${CSI}?1049h${CSI}H alt top${CSI}5;3Hmiddle`),
    screen(20),
    resize(30, 100, 40),
    screen(40),
    out(50, `${CSI}30;1Hlow`),
    resize(60, 30, 6),
    screen(70),
    out(80, `${CSI}?1049l`),
    screen(90),
  ];
  return { name: 'alt-screen-resize', cmd: ['vim'], cols: 80, rows: 24, steps, end: 500 };
}

/** 種付きの疑似乱数 (mulberry32)。 */
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * 画面の意味が決まっている範囲の制御列を、種付きの乱数で混ぜた出力。
 * 端末の実装ごとに解釈が分かれる列 (DEC の特殊モード、同期更新など) は入れない。
 * 小さな画面・小さなスクロールバック・resize を混ぜて、折り返し / スクロール / 再配置の境界を踏ませる。
 */
function fuzz(seed, { cols = 80, rows = 24, scrollback = 5000, resizes = false, chunks = 60 } = {}) {
  const r = rng(seed);
  const pick = (a) => a[Math.floor(r() * a.length)];
  const n = (max) => 1 + Math.floor(r() * max);
  const words = ['alpha', 'beta', 'gamma ', ' delta', 'x', '日本', 'テスト', 'e\u0301', '0123456789', '--', '| |', '═══', '✅', '😀', 'a\u0308b', '  ', 'ｱｲｳ'];
  const pieces = [
    () => pick(words),
    () => pick(words),
    () => pick(words),
    () => '\r\n',
    () => '\r\n',
    () => pick(['\r', '\n', '\b', '\t', '\x0b', '\x0c']),
    () => `${CSI}${n(5)}A`,
    () => `${CSI}${n(5)}B`,
    () => `${CSI}${n(9)}C`,
    () => `${CSI}${n(9)}D`,
    () => `${CSI}${n(rows)};${n(cols)}H`,
    () => `${CSI}${n(cols)}G`,
    () => `${CSI}${n(rows)}d`,
    () => `${CSI}${n(3)}E`,
    () => `${CSI}${n(3)}F`,
    () => `${CSI}${pick(['', '0', '2', '3'])}J`,
    () => `${CSI}${pick(['', '0', '1', '2'])}K`,
    () => `${CSI}${n(3)}L`,
    () => `${CSI}${n(3)}M`,
    () => `${CSI}${n(4)}P`,
    () => `${CSI}${n(4)}@`,
    () => `${CSI}${n(4)}X`,
    () => `${CSI}${n(3)}S`,
    () => `${CSI}${n(3)}T`,
    () => `${CSI}${n(3)}I`,
    () => `${CSI}${n(3)}Z`,
    () => `${CSI}${n(3)}b`,
    () => `${CSI}${pick(['0', '3', '0g'])}g`,
    () => `${CSI}${pick(['0', '1', '31', '42', '38;5;99', '7'])}m`,
    () => `${CSI}${n(10)};${n(rows)}r`,
    () => `${CSI}r`,
    () => `${CSI}${pick(['4', '20'])}${pick(['h', 'l'])}`,
    () => `${CSI}?${pick(['6', '7', '45'])}${pick(['h', 'l'])}`,
    () => `${CSI}s`,
    () => `${CSI}u`,
    () => `${ESC}7`,
    () => `${ESC}8`,
    () => `${ESC}M`,
    () => `${ESC}D`,
    () => `${ESC}E`,
    () => `${ESC}H`,
    () => `${ESC}(0qxlk${ESC}(B`,
    () => pick(['\x0e', '\x0f']),
    // resize と代替画面を混ぜない: xterm.js は、空の代替画面を resize してから使うと、行リストの長さと ybase が食い違った
    // 状態になり、画面の外の古い行を読む (実装の内部状態に依存した結果で、再現する意味がない)。
    ...(resizes ? [] : [() => pick([`${CSI}?1049h`, `${CSI}?1049l`, `${CSI}?47h`, `${CSI}?47l`, `${CSI}?1048h`, `${CSI}?1048l`])]),
    () => `${CSI}!p`,
    () => title(pick(['a', 'build', '⠋ job', 'x y'])),
  ];
  const steps = [];
  let t = 0;
  for (let i = 0; i < chunks; i++) {
    let chunk = '';
    for (let j = 0, k = n(8); j < k; j++) chunk += pick(pieces)();
    t += 10;
    steps.push(out(t, chunk));
    if (resizes && r() < 0.12) {
      t += 5;
      steps.push(resize(t, n(Math.max(2, cols + 8)), n(rows + 6)));
    }
    if (process.env.MISAO_FUZZ_EVERY ? true : i % 6 === 5) steps.push(screen(t + 1));
  }
  steps.push(screen(t + 100));
  return { name: `fuzz-${seed}`, cmd: ['bash'], cols, rows, scrollback, steps, end: t + 500 };
}

export const fuzzScenarios = (count) => Array.from({ length: count }, (_, i) => fuzz(1000 + i, [{ cols: 12, rows: 6, scrollback: 8 }, { cols: 30, rows: 10, scrollback: 20, resizes: true }, { cols: 5, rows: 3, scrollback: 4, resizes: true }, {}][i % 4]));

export const scenarios = [
  claudeSession(),
  claudeQuestion(),
  claudeApiError(),
  shellWrappedClaude(),
  codexSession(),
  bytesOnly(),
  titleOnly(),
  exitFlow(),
  plainTerminal(),
  wideChars(),
  altScreen(),
  resizeFlow(),
  altScreenResize(),
  fuzz(1),
  fuzz(2),
  fuzz(3, { cols: 12, rows: 6, scrollback: 8 }),
  fuzz(4, { cols: 12, rows: 6, scrollback: 8 }),
  fuzz(5, { cols: 30, rows: 10, scrollback: 20, resizes: true }),
  fuzz(6, { cols: 30, rows: 10, scrollback: 20, resizes: true }),
  fuzz(7, { cols: 5, rows: 3, scrollback: 4, resizes: true }),
  fuzz(8, { cols: 5, rows: 3, scrollback: 4, resizes: true }),
].map((s) => ({ scrollback: 5000, ...s }));

/** 前面プロセス名の差分テスト用 (foreground.ts の純関数)。 */
export const foregroundArgv = [
  ['/usr/bin/sleep', '30'],
  ['bash'],
  ['node', '/x/codex'],
  ['node', '--inspect', '/x/a.mjs'],
  ['python3.12', '-u', 'x.py'],
  ['node', '-r', 'dotenv/config', 'app.js'],
  ['node', '--import', 'tsx', 'a.ts'],
  ['node', '--import=tsx', '--conditions', 'dev', 'b.mjs'],
  ['python3', '-W', 'ignore', '-X', 'dev', 'x.py'],
  ['python3', '-c', 'import os'],
  ['node', '-e', '1'],
  ['node', '--eval=1'],
  ['node', '-p', '1 + 1'],
  ['python3', '-m', 'http.server'],
  ['python3', '-m'],
  ['bun', 'run', 'dev.ts'],
  ['bun', 'x', 'tool'],
  ['bun', '--preload', 'p.ts', 'run', 's.ts'],
  ['node', '--', 'x.js'],
  ['node', '--'],
  ['node'],
  ['ruby', '-I', 'lib', 'a.rb'],
  ['perl', '-E', 'say 1'],
  ['perl', 'x.pl'],
  ['/bin/sh', '-c', 'node x.js'],
  ['node', 'a b.js'],
  ['node', '/x/.hidden.tar.gz'],
  ['/usr/bin/\u202Eevil\u200B'],
  ['x'.repeat(100)],
  ['日本語コマンド'],
  [''],
  [],
];

export const foregroundStats = [
  '123 (bash) S 100 123 123 34816 4567 4194304 1 0',
  '123 (a) (b) S 100 123 123 34816 789 4194304 1 0',
  '123 (bash) S 100 123 123 0 -1 4194304 1 0',
  '123 (bash) S 100 123 123 0 0 4194304 1 0',
  '123 (bash) S 100 123 123 0 12x 4194304 1 0',
  'garbage',
  '',
  '1 (x) S 1',
];
