export type TitleVerdict = 'working' | 'idle' | null;

/** 汎用のスピナー: 点字 / ◐◑◒◓ / ✻✶✽✢∗ で始まるタイトル。 */
const SPINNER_TITLE_RE = /^[⠀-⣿◐◑◒◓✻✶✽✢∗] /;
/** スピナーは約 1 秒で切り替わる。これだけ更新が無ければ止まったとみなして意見を持たない。 */
const SPINNER_STALE_MS = 3000;

/**
 * 判定の title 段。OSC タイトルの見た目だけで working / idle を決める。時刻は引数で受け取る。
 * スピナーを一度でも見た pane は、タイトルを自分で更新するエージェントとみなし、
 * スピナーでない空でないタイトルを idle と判定する。
 */
export class TitleActivity {
  private title = '';
  private updatedAt = 0;
  private hasSeenSpinner = false;

  setTitle(title: string, now: number): void {
    this.title = title;
    this.updatedAt = now;
    if (SPINNER_TITLE_RE.test(title)) this.hasSeenSpinner = true;
  }

  verdict(now: number): TitleVerdict {
    if (SPINNER_TITLE_RE.test(this.title)) return now - this.updatedAt < SPINNER_STALE_MS ? 'working' : null;
    return this.hasSeenSpinner && this.title !== '' ? 'idle' : null;
  }
}
