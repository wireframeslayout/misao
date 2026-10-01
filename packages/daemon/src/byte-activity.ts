export type ByteVerdict = 'working' | 'idle' | null;

/** 出力量を数えるスライディングウィンドウの長さ。 */
const WINDOW_MS = 3000;
/** ウィンドウ内の合計がこの量以上のティックを「活動あり」とする。 */
const ACTIVE_THRESHOLD_BYTES = 200;
/** idle → working に必要な、活動ありのティックの連続数 (エコーの除外)。 */
const ACTIVE_CONSECUTIVE_TICKS = 2;
/** 最後の活動ありのティックからこの時間静かなら idle。 */
const IDLE_AFTER_MS = 5000;
/** 入力の後の出力はキー入力のエコー / 入力欄の再描画なので数えない。 */
const INPUT_GRACE_MS = 500;
/** resize の後の出力は全画面の再描画なので数えない。 */
const RESIZE_GRACE_MS = 800;

interface Sample {
  ts: number;
  bytes: number;
}

/**
 * 判定の最後の段 (bytes)。出力量だけで working / idle を決める。
 * 時刻は引数で受け取る (タイマーは持たない)。tick は呼び出し側が一定間隔で呼ぶ。
 * 開いてから最初の活動が無いまま IDLE_AFTER_MS が過ぎるまでは意見なし (null)。
 */
export class ByteActivity {
  private samples: Sample[] = [];
  private state: ByteVerdict = null;
  private aboveStreak = 0;
  private hasFreshBytes = false;
  private lastAboveTs: number;
  private inputGraceUntil = 0;
  private resizeGraceUntil = 0;

  constructor(startedAt: number) {
    this.lastAboveTs = startedAt;
  }

  notifyInput(now: number): void {
    this.inputGraceUntil = now + INPUT_GRACE_MS;
  }

  notifyResize(now: number): void {
    this.resizeGraceUntil = now + RESIZE_GRACE_MS;
  }

  record(bytes: number, now: number): void {
    if (now < this.resizeGraceUntil || now < this.inputGraceUntil) return;
    this.samples.push({ ts: now, bytes });
    this.hasFreshBytes = true;
  }

  /** 一定間隔 (1 秒) で呼ぶ。現在の意見を返す。 */
  tick(now: number): ByteVerdict {
    const cutoff = now - WINDOW_MS;
    this.samples = this.samples.filter((s) => s.ts >= cutoff);
    const sum = this.samples.reduce((acc, s) => acc + s.bytes, 0);
    const isAbove = sum >= ACTIVE_THRESHOLD_BYTES && this.hasFreshBytes;
    this.hasFreshBytes = false;
    this.aboveStreak = isAbove ? this.aboveStreak + 1 : 0;
    if (isAbove) this.lastAboveTs = now;

    if (this.state !== 'working') {
      if (this.aboveStreak >= ACTIVE_CONSECUTIVE_TICKS) this.state = 'working';
      else if (now - this.lastAboveTs >= IDLE_AFTER_MS) this.state = 'idle';
    } else if (!isAbove && now - this.lastAboveTs >= IDLE_AFTER_MS) {
      this.state = 'idle';
    }
    return this.state;
  }
}
