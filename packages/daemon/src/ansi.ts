import { StringDecoder } from 'node:string_decoder';

type State = 'ground' | 'esc' | 'escInter' | 'csi' | 'osc' | 'oscEsc' | 'str' | 'strEsc';

/**
 * ANSI 除去 + 行組み立て。チャンク境界をまたぐ状態を保持する。
 * - CSI / OSC (BEL or ST 終端) / DCS,SOS,PM,APC (ST 終端) / 2 文字 ESC を除去
 * - \n \r \t 以外の C0 と DEL は捨てる
 * - `\r\n` は改行。単独 `\r` は「以降に書かれた文字で行を上書き」= 現在行バッファの破棄
 * - 行は `\n` で確定する。未確定の行は pending で覗ける。
 * - 行の文字列は 1 文字ずつ連結しない (V8 の ConsString が木のまま残るため)。
 *   デコード済みの連続区間を slice で切り出して parts に溜め、確定時に join して平坦な文字列にする。
 */
export class AnsiLineAssembler {
  private readonly decoder = new StringDecoder('utf8');
  private state: State = 'ground';
  /** 現在行のうち確定済みの断片 (push をまたぐ分と、途中で途切れた区間)。 */
  private parts: string[] = [];
  /** 現在の push のテキストと、そのうち現在行に属する直近の連続区間 [runStart, runEnd)。 */
  private src = '';
  private runStart = 0;
  private runEnd = 0;
  /** step 中の文字の src 上の開始位置。 */
  private pos = 0;
  private pendingCr = false;

  get pending(): string {
    this.closeRun();
    return this.parts.join('');
  }

  push(chunk: Buffer | string): string[] {
    const text = typeof chunk === 'string' ? chunk : this.decoder.write(chunk);
    const out: string[] = [];
    this.src = text;
    this.runStart = this.runEnd = 0;
    for (let i = 0; i < text.length; ) {
      const ch = String.fromCodePoint(text.codePointAt(i)!);
      this.pos = i;
      this.step(ch, out);
      i += ch.length;
    }
    this.closeRun();
    this.src = '';
    return out;
  }

  private closeRun(): void {
    if (this.runEnd > this.runStart) this.parts.push(this.src.slice(this.runStart, this.runEnd));
    this.runStart = this.runEnd = 0;
  }

  private resetLine(): void {
    this.parts = [];
    this.runStart = this.runEnd = 0;
  }

  private appendChar(ch: string): void {
    if (this.runEnd > this.runStart && this.runEnd !== this.pos) this.closeRun();
    if (this.runEnd === this.runStart) this.runStart = this.pos;
    this.runEnd = this.pos + ch.length;
  }

  private step(ch: string, out: string[]): void {
    const c = ch.codePointAt(0)!;
    // 実端末は C0 を即時実行する。エスケープ列の途中でも改行 / CR は ground で処理する。
    if ((c === 0x0a || c === 0x0d) && (this.state === 'esc' || this.state === 'escInter' || this.state === 'csi')) {
      this.state = 'ground';
    }
    switch (this.state) {
      case 'ground':
        return this.ground(ch, c, out);
      case 'esc':
        if (ch === '[') this.state = 'csi';
        else if (ch === ']') this.state = 'osc';
        else if (ch === 'P' || ch === 'X' || ch === '^' || ch === '_') this.state = 'str';
        else if (c >= 0x20 && c <= 0x2f) this.state = 'escInter';
        else if (c === 0x1b) this.state = 'esc';
        else if (c >= 0x30 && c <= 0x7e) this.state = 'ground';
        else {
          // 範囲外 (C0 / 非 ASCII など): シーケンスを打ち切り、その文字を通常文字として再解釈
          this.state = 'ground';
          this.step(ch, out);
        }
        return;
      case 'escInter':
        if (c >= 0x30 && c <= 0x7e) this.state = 'ground';
        else if (c === 0x1b) this.state = 'esc';
        else if (c === 0x18 || c === 0x1a) this.state = 'ground';
        return;
      case 'csi':
        if (c >= 0x40 && c <= 0x7e) this.state = 'ground';
        else if (c === 0x1b) this.state = 'esc';
        else if (c === 0x18 || c === 0x1a) this.state = 'ground';
        return;
      case 'osc':
        if (c === 0x07) this.state = 'ground';
        else if (c === 0x1b) this.state = 'oscEsc';
        else if (c === 0x18 || c === 0x1a) this.state = 'ground';
        return;
      case 'str':
        if (c === 0x1b) this.state = 'strEsc';
        else if (c === 0x18 || c === 0x1a) this.state = 'ground';
        return;
      case 'oscEsc':
      case 'strEsc':
        // ESC \ (ST) で終端。それ以外は新しい ESC シーケンスの開始として再解釈する。
        if (ch === '\\') this.state = 'ground';
        else {
          this.state = 'esc';
          this.step(ch, out);
        }
        return;
    }
  }

  private ground(ch: string, c: number, out: string[]): void {
    if (c === 0x1b) {
      this.state = 'esc';
    } else if (c === 0x0a) {
      this.closeRun();
      out.push(this.parts.join(''));
      this.parts = [];
      this.pendingCr = false;
    } else if (c === 0x0d) {
      this.pendingCr = true;
    } else if (c === 0x09 || (c >= 0x20 && c !== 0x7f)) {
      if (this.pendingCr) {
        this.resetLine();
        this.pendingCr = false;
      }
      this.appendChar(ch);
    }
  }
}
