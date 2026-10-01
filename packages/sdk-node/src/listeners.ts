/** `add(cb) → off` 形式のリスナー集合。 */
export class Listeners<T> {
  private readonly callbacks = new Set<(value: T) => void>();

  add(callback: (value: T) => void): () => void {
    this.callbacks.add(callback);
    return () => {
      this.callbacks.delete(callback);
    };
  }

  emit(value: T): void {
    for (const callback of [...this.callbacks]) callback(value);
  }
}
