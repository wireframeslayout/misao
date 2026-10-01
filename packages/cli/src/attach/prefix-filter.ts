import type { KeyBindings } from '../config/index.js';

export type PrefixAction = 'detach' | 'next' | 'prev' | 'list';

export interface PrefixResult {
  /** ペインへ転送するバイト列。 */
  forward: Buffer;
  /** prefix + アクションキーが押された。以後の入力は処理しない。 */
  action: PrefixAction | null;
}

const ACTIONS: readonly PrefixAction[] = ['detach', 'next', 'prev', 'list'];

/**
 * prefix キーの状態機械。prefix の次のキーがアクションキーなら action、prefix をもう一度なら
 * prefix の 1 バイトを転送し、それ以外のキーは捨てる。状態は feed をまたいで保つ。
 */
export class PrefixFilter {
  private isPending = false;

  constructor(private readonly keys: KeyBindings) {}

  feed(data: Buffer): PrefixResult {
    const forward: number[] = [];
    for (const byte of data) {
      if (!this.isPending) {
        if (byte === this.keys.prefix) this.isPending = true;
        else forward.push(byte);
        continue;
      }
      this.isPending = false;
      const action = ACTIONS.find((a) => this.keys[a] === byte);
      if (action !== undefined) return { forward: Buffer.from(forward), action };
      if (byte === this.keys.prefix) forward.push(byte);
    }
    return { forward: Buffer.from(forward), action: null };
  }
}
