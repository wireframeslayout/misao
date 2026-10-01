export interface RingEntry<T> {
  seq: number;
  ts: string;
  size: number;
  item: T;
}

/**
 * seq 付きリングバッファ。容量は size 単位 (バイト数や件数) で制限する。
 * 1 件が容量を超えていても直近 1 件は必ず残す。
 */
export class SeqRing<T> {
  private entries: RingEntry<T>[] = [];
  private start = 0;
  private total = 0;
  private headSeq = 0;

  constructor(private readonly capacity: number) {}

  get head(): number {
    return this.headSeq;
  }

  get length(): number {
    return this.entries.length - this.start;
  }

  /** 保持している最古の seq。空なら head + 1。 */
  get oldest(): number {
    return this.length > 0 ? this.entries[this.start]!.seq : this.headSeq + 1;
  }

  push(item: T, size: number, ts: string): number {
    const seq = ++this.headSeq;
    this.entries.push({ seq, ts, size, item });
    this.total += size;
    while (this.total > this.capacity && this.length > 1) {
      this.total -= this.entries[this.start]!.size;
      this.entries[this.start] = undefined as never;
      this.start++;
    }
    if (this.start > 1024 && this.start * 2 > this.entries.length) {
      this.entries = this.entries.slice(this.start);
      this.start = 0;
    }
    return seq;
  }

  /** seq のエントリ。保持範囲外 (追い出し済み・head より先) なら undefined。O(1)。 */
  entry(seq: number): RingEntry<T> | undefined {
    if (seq < this.oldest || seq > this.headSeq) return undefined;
    return this.entries[this.start + (seq - this.oldest)];
  }

  /** seq > since のエントリ。 */
  since(since: number): RingEntry<T>[] {
    // seq は連続なので添字を直接計算できる。
    const first = this.oldest;
    const from = Math.max(since + 1, first);
    if (from > this.headSeq) return [];
    return this.entries.slice(this.start + (from - first));
  }

  /** since が保持範囲より古い、または head より先 (= 送信側が再起動して seq が巻き戻った) なら欠落扱い。 */
  hasGap(since: number): boolean {
    return since < this.oldest - 1 || since > this.headSeq;
  }
}
