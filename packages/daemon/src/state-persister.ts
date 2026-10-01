import { checkConsistency } from './persistence.js';
import type { PersistedState } from './persistence.js';

/** サイズ変更の保存を遅らせる時間。連続した resize を 1 回の保存にまとめる。 */
const SIZE_SAVE_DELAY_MS = 1000;

export interface StatePersisterOptions {
  /** 現在の状態。保存のたびに呼ぶ。 */
  snapshot: () => PersistedState;
  save: (state: PersistedState) => void;
  log: (msg: string) => void;
}

/**
 * 保存のタイミングを決める。
 * - saveNow: 定義・ラベルなど失うと困る変更。同期で保存し、失敗は例外にする。
 * - saveSoon: cols / rows のような再起動後の表示用の付帯情報。遅らせて保存し、失敗はログだけ (RPC は落とさない)。
 */
export class StatePersister {
  private timer: NodeJS.Timeout | undefined;

  constructor(private readonly opts: StatePersisterOptions) {}

  /** 壊れた状態 (存在しない window を指す pane など) は書かずに例外にする。 */
  saveNow(): void {
    this.cancelPending();
    const state = this.opts.snapshot();
    const issues = checkConsistency(state);
    if (issues.length > 0) throw new Error(`refusing to save inconsistent state: ${issues.join('; ')}`);
    this.opts.save(state);
  }

  saveSoon(): void {
    if (this.timer) return;
    this.timer = setTimeout(() => this.saveQuietly(), SIZE_SAVE_DELAY_MS);
    this.timer.unref();
  }

  /** 遅らせている保存があれば今すぐ行う (shutdown 用)。失敗はログだけ。 */
  flush(): void {
    if (this.timer) this.saveQuietly();
  }

  private saveQuietly(): void {
    try {
      this.saveNow();
    } catch (e) {
      this.opts.log(`failed to persist state: ${(e as Error).message}`);
    }
  }

  private cancelPending(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
  }
}
