import type { IBufferCell, Terminal } from '@xterm/headless';

/** xterm.js の書き込みは非同期キューなので、キューが空になる (= これまでの入力が反映される) のを待つ。 */
export function flushTerminal(term: Terminal): Promise<void> {
  return new Promise((resolve) => term.write('', resolve));
}

export function viewportText(term: Terminal): string {
  const buf = term.buffer.active;
  const lines: string[] = [];
  for (let y = 0; y < term.rows; y++) {
    lines.push(buf.getLine(buf.viewportY + y)?.translateToString(true) ?? '');
  }
  return lines.join('\n');
}

function colorParams(mode: 'fg' | 'bg', cell: IBufferCell): string[] {
  const isFg = mode === 'fg';
  const isDefault = isFg ? cell.isFgDefault() : cell.isBgDefault();
  if (isDefault) return [];
  const value = isFg ? cell.getFgColor() : cell.getBgColor();
  const base = isFg ? 30 : 40;
  if (isFg ? cell.isFgRGB() : cell.isBgRGB()) {
    return [String(base + 8), '2', String((value >> 16) & 255), String((value >> 8) & 255), String(value & 255)];
  }
  if (value < 8) return [String(base + value)];
  if (value < 16) return [String(base + 60 + (value - 8))];
  return [String(base + 8), '5', String(value)];
}

/** セルの属性を、リセット込みの SGR 列 (`\x1b[0;...m`) にする。属性が既定なら `\x1b[0m`。 */
function sgrFor(cell: IBufferCell): string {
  const p: string[] = ['0'];
  if (cell.isBold()) p.push('1');
  if (cell.isDim()) p.push('2');
  if (cell.isItalic()) p.push('3');
  if (cell.isUnderline()) p.push('4');
  if (cell.isBlink()) p.push('5');
  if (cell.isInverse()) p.push('7');
  if (cell.isInvisible()) p.push('8');
  if (cell.isStrikethrough()) p.push('9');
  p.push(...colorParams('fg', cell), ...colorParams('bg', cell));
  return `\x1b[${p.join(';')}m`;
}

/**
 * ヘッドレス端末の「現在の見た目」を再生用エスケープ列にシリアライズする。
 * viewport のみ (scrollback は含めない)。alt screen なら alt に入ってから描く。
 * 自前実装 (@xterm/addon-serialize 不使用)。
 */
export function serializeSnapshot(term: Terminal): string {
  const buf = term.buffer.active;
  const out: string[] = [];
  if (buf.type === 'alternate') out.push('\x1b[?1049h');
  out.push('\x1b[0m\x1b[2J\x1b[H');

  const cell = buf.getNullCell();
  for (let y = 0; y < term.rows; y++) {
    const line = buf.getLine(buf.viewportY + y);
    if (!line) continue;
    // 末尾の「既定属性の空白」は書かない (行末は EL で消す)。
    let last = line.length - 1;
    for (; last >= 0; last--) {
      const c = line.getCell(last, cell);
      if (!c) continue;
      if (c.getWidth() === 0) continue;
      const ch = c.getChars();
      if (ch !== '' && ch !== ' ') break;
      if (!c.isAttributeDefault()) break;
    }
    let row = `\x1b[${y + 1};1H`;
    let cur = '';
    for (let x = 0; x <= last; x++) {
      const c = line.getCell(x, cell);
      if (!c || c.getWidth() === 0) continue;
      const sgr = sgrFor(c);
      if (sgr !== cur) {
        row += sgr;
        cur = sgr;
      }
      row += c.getChars() || ' ';
    }
    out.push(row + '\x1b[0m\x1b[K');
  }

  const m = term.modes;
  if (m.applicationCursorKeysMode) out.push('\x1b[?1h');
  if (m.bracketedPasteMode) out.push('\x1b[?2004h');
  const mouse = { none: '', x10: '\x1b[?9h', vt200: '\x1b[?1000h', drag: '\x1b[?1002h', any: '\x1b[?1003h' }[
    m.mouseTrackingMode
  ];
  if (mouse) out.push(mouse);
  out.push(`\x1b[${buf.cursorY + 1};${buf.cursorX + 1}H`);
  return out.join('');
}
