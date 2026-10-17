// Rust 版の profile (crates/misao-daemon/src/profiles/*.json) と突き合わせるための、JS による参照実装。
// 規則の出どころは AZITO の画面規則 (agentScreenRules.ts の CLAUDE_SCREEN_RULES / CODEX_SCREEN_RULES と
// splitPromptBox)。Rust 側はこれをデータ (JSON) として持つ。ここは「規則をコードで書いたらこうなる」という
// 別実装で、判定の差分テストで両者が同じ列を出すことを確かめる。
//
// ProfileVerdict は working / blocked / idle / null。AZITO の skip と error は「意見なし」(null) にするが、
// 優先順位の競り合いには参加する (上位の skip / error が下位の規則を黙らせる)。

const SPINNER = '[*·✢✳✶✻✽]';
const SPINNER_LINE_RE = new RegExp(`^\\s*${SPINNER}\\s+\\S+.*…(?:\\s+\\(\\d+[smh](?:\\s|·)|\\s*$)`);
const INTERRUPT_LINE_RE = /^\s*[⏸⏵].*esc to interrupt(?:\s|·|$)/i;
const PROMPT_LINE_RE = /^\s*❯/;
const OPTION_CURSOR_RE = /^\s*❯\s*\d+\./;

const lower = (lines) => lines.map((l) => l.toLowerCase());
const anyLine = (lines, re) => lines.some((l) => re.test(l));
const text = (lines) => lower(lines).join('\n');

const CLAUDE_RULES = [
  { id: 'transcript_viewer', state: 'skip', priority: 100, region: 'all', test: (ls) => text(ls.slice(-3)).includes('showing detailed transcript') },
  {
    id: 'blocked_form',
    state: 'blocked',
    priority: 90,
    region: 'all',
    test: (ls) => {
      const t = text(ls);
      return t.includes('esc to cancel') && (t.includes('enter to confirm') || t.includes('enter to select'));
    },
  },
  {
    id: 'permission_prompt',
    state: 'blocked',
    priority: 85,
    region: 'all',
    test: (ls) => {
      const t = text(ls);
      return t.includes('do you want to proceed?') || t.includes('requires approval') || (/(❯|1\.)\s*yes/.test(t) && /2\.\s*no/.test(t));
    },
  },
  {
    id: 'api_error',
    state: 'error',
    priority: 60,
    region: 'above',
    test: (ls) => {
      const t = text(ls.slice(-4));
      if (t.includes('retrying in')) return false;
      return t.includes('api error') && (t.includes('request timed out') || t.includes('overloaded') || t.includes('connection error') || t.includes('service unavailable'));
    },
  },
  { id: 'interrupt_line', state: 'working', priority: 70, region: 'all', test: (ls) => anyLine(ls, INTERRUPT_LINE_RE) },
  { id: 'spinner_line', state: 'working', priority: 70, region: 'above', test: (ls) => anyLine(ls, SPINNER_LINE_RE) },
  {
    id: 'background_work',
    state: 'working',
    priority: 65,
    region: 'above',
    test: (ls) => {
      const t = text(ls);
      return /waiting for \d+ background agents/.test(t) || /\d+ mcp tasks? still running/.test(t) || /\(\S*\d+\S*\s*tokens\s*\)/i.test(t);
    },
  },
  {
    id: 'prompt_box_idle',
    state: 'idle',
    priority: 50,
    region: 'promptBox',
    test: (ls) => anyLine(ls, PROMPT_LINE_RE) && !anyLine(ls, OPTION_CURSOR_RE) && !text(ls).includes('enter to select') && !text(ls).includes('esc to cancel'),
  },
];

const CODEX_RULES = [
  {
    id: 'action_required_title',
    state: 'blocked',
    priority: 95,
    region: 'title',
    test: (ls) => text(ls).includes('action required'),
  },
  {
    id: 'confirm',
    state: 'blocked',
    priority: 90,
    region: 'all',
    test: (ls) => {
      const t = text(ls);
      return t.includes('press enter to confirm or esc to cancel') || t.includes('allow command?') || t.includes('enter to submit');
    },
  },
  {
    id: 'yes_no',
    state: 'blocked',
    priority: 85,
    region: 'all',
    test: (ls) => {
      const t = text(ls);
      return t.includes('[y/n]') || (t.includes('do you want to') && t.includes('yes'));
    },
  },
  { id: 'working', state: 'working', priority: 70, region: 'above', test: (ls) => /esc to interrupt/.test(text(ls)) },
];

export function splitPromptBox(rows, maxAbove = 12) {
  const isRule = (l) => /^\s*─{10,}\s*$/.test(l);
  let bottom = -1;
  let top = -1;
  for (let i = rows.length - 1; i >= 0; i--) {
    if (!isRule(rows[i])) continue;
    if (bottom < 0) {
      bottom = i;
      continue;
    }
    top = i;
    break;
  }
  if (top < 0) return { above: rows.filter((l) => l.trim() !== '').slice(-maxAbove), promptBox: [], below: [] };
  return {
    above: rows.slice(0, top).filter((l) => l.trim() !== '').slice(-maxAbove),
    promptBox: rows.slice(top + 1, bottom),
    below: rows.slice(bottom + 1).filter((l) => l.trim() !== ''),
  };
}

function classify(rules, screen) {
  const input = splitPromptBox(screen.rows);
  const all = [...input.above, ...input.promptBox, ...input.below];
  const regions = { above: input.above, promptBox: input.promptBox, below: input.below, all, title: screen.title === '' ? [] : [screen.title] };
  let best;
  for (const r of rules) {
    const lines = regions[r.region];
    if (lines.length === 0) continue;
    if (r.test(lines) && (!best || r.priority > best.priority)) best = r;
  }
  if (!best) return null;
  return best.state === 'working' || best.state === 'blocked' || best.state === 'idle' ? best.state : null;
}

const isAssignment = (w) => /^[A-Za-z_][A-Za-z0-9_]*=/.test(w);
/** パスの最後の要素 (/ と \ で区切る) から .exe (大文字小文字を問わない) を除いたもの。Rust 側の base_name と同じ。 */
const baseName = (s) => (s ?? '').split(/[\\/]/).pop().replace(/\.exe$/i, '');

/** cmd[0] の basename (先頭の env ラッパーは飛ばす)。Rust 側の program_name と同じ規則。 */
export function programName(cmd) {
  if (baseName(cmd[0]) !== 'env') return baseName(cmd[0]);
  let i = 1;
  while (i < cmd.length) {
    const a = cmd[i];
    if (a === '-u' || a === '--unset' || a === '-C' || a === '--chdir') i += 2;
    else if (a.startsWith('-') || isAssignment(a)) i += 1;
    else break;
  }
  return i < cmd.length ? baseName(cmd[i]) : '';
}

const SHELLS = ['sh', 'bash', 'zsh', 'dash', 'ksh', 'fish'];
const COMMAND_PREFIXES = ['exec', 'command', 'nohup', 'env'];

/** 起動コマンドが実行しようとしているプログラム名。シェルへ文字列で渡すときは、その各コマンドの先頭の語。Rust 側の command_programs と同じ。 */
export function commandPrograms(cmd) {
  const launcher = programName(cmd);
  if (!SHELLS.includes(launcher)) return [launcher];
  let script;
  for (let i = 1; i < cmd.length; i++) {
    if (cmd[i].startsWith('-') && !cmd[i].startsWith('--') && cmd[i].includes('c')) {
      script = cmd[i + 1];
      break;
    }
  }
  if (script === undefined) return [launcher];
  return script
    .split(/[&|;\n]/)
    .map((segment) => segment.split(/\s+/).filter((w) => w !== '').find((w) => !isAssignment(w) && !COMMAND_PREFIXES.includes(w) && !w.startsWith('-')))
    .filter((w) => w !== undefined)
    .map((w) => baseName(w.replace(/^['"]+|['"]+$/g, '')));
}

export const referenceProfiles = [
  { name: 'claude', matches: (cmd) => commandPrograms(cmd).includes('claude'), classify: (screen) => classify(CLAUDE_RULES, screen) },
  { name: 'codex', matches: (cmd) => commandPrograms(cmd).includes('codex'), classify: (screen) => classify(CODEX_RULES, screen) },
];
