export interface SizeDecision {
  cols: number;
  rows: number;
  /** このサイズを決めたクライアント。clientId なしの resize では null。 */
  owner: string | null;
}

/**
 * 「最後に操作したクライアントのサイズを優先する」調停。pty には依存せず、
 * 適用すべきサイズを返すだけ（適用は呼び出し側）。
 */
export class SizeArbiter {
  /** 挿入順 = 操作の新しさ。 */
  private readonly sizes = new Map<string, { cols: number; rows: number }>();
  private ownerId: string | null = null;

  get owner(): string | null {
    return this.ownerId;
  }

  /** clientId のサイズを記録し、そのクライアントを所有者にする。clientId なしは所有者を null にする。 */
  record(clientId: string | null, cols: number, rows: number): SizeDecision {
    if (clientId) {
      this.sizes.delete(clientId);
      this.sizes.set(clientId, { cols, rows });
    }
    this.ownerId = clientId;
    return { cols, rows, owner: clientId };
  }

  /** clientId が所有者でなく、サイズを記録済みなら、そのサイズへ戻す。変更なしは null。 */
  claim(clientId: string): SizeDecision | null {
    const size = this.sizes.get(clientId);
    if (!size || this.ownerId === clientId) return null;
    return this.record(clientId, size.cols, size.rows);
  }

  /**
   * クライアントの離脱。所有者だった場合は、残るクライアントのうち最後に操作したものの
   * サイズを返す（なければ null）。
   */
  forget(clientId: string): SizeDecision | null {
    const wasOwner = this.ownerId === clientId;
    this.sizes.delete(clientId);
    if (!wasOwner) return null;
    this.ownerId = null;
    const last = [...this.sizes.entries()].pop();
    if (!last) return null;
    return this.record(last[0], last[1].cols, last[1].rows);
  }
}
