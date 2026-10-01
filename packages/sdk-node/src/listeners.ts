/**
 * `add(cb) → off` 形式のリスナー集合。コールバックは 1 件ずつ try/catch で呼び、
 * 例外は report に渡して残りの配信を続ける (利用側のバグで SDK の内部処理を止めない)。
 */
export class Listeners<T> {
  private readonly callbacks = new Set<(value: T) => void>();

  constructor(private readonly report: (error: unknown) => void) {}

  get size(): number {
    return this.callbacks.size;
  }

  add(callback: (value: T) => void): () => void {
    this.callbacks.add(callback);
    return () => {
      this.callbacks.delete(callback);
    };
  }

  emit(value: T): void {
    for (const callback of [...this.callbacks]) {
      try {
        callback(value);
      } catch (error) {
        this.report(error);
      }
    }
  }
}
