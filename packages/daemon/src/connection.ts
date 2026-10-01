import type * as net from 'node:net';
import { ErrorCode, LineSplitter, encodeMessage } from '@misao/protocol';
import type { RpcMessage } from '@misao/protocol';

/** attach (pane.output の raw) と応答で、これ以上送信キューが溜まった接続 (遅い consumer) は切る。行・イベントの購読は pull 型でこの上限を使わない。 */
export const MAX_WRITABLE_BYTES = 16 * 1024 * 1024;

/** 購読ストリームはキューがこれ未満の間だけ送る。超えたら drain を待つ (送信キューを小さく保つ)。 */
export const STREAM_HIGH_WATER_BYTES = 1024 * 1024;

export interface ConnectionHandlers {
  /** 1 行ぶんの JSON をパースした値（オブジェクトとは限らない）。 */
  onMessage(conn: Connection, value: unknown): void;
  onClose(conn: Connection): void;
}

export class Connection {
  private readonly splitter = new LineSplitter();
  /** paneId → clientId */
  readonly attachments = new Map<string, { clientId: string; off: () => void }>();
  /** `${stream}:${paneId}` → off。同じキーの再購読は既存を置き換える (二重配信の防止)。 */
  readonly subscriptions = new Map<string, () => void>();
  closed = false;
  /** 購読ストリームの drain 待ち。ソケットには emitDrain 1 つだけを登録する (購読数でリスナーを増やさない)。 */
  private readonly drainListeners = new Set<() => void>();
  private rejecting = false;

  constructor(
    readonly socket: net.Socket,
    handlers: ConnectionHandlers,
  ) {
    socket.on('data', (chunk: Buffer) => {
      if (this.rejecting) return;
      let lines: string[];
      try {
        lines = this.splitter.push(chunk);
      } catch (e) {
        this.reject((e as Error).message);
        return;
      }
      for (const line of lines) this.dispatchLine(line, handlers);
    });
    socket.on('error', () => socket.destroy());
    socket.on('close', () => {
      this.closed = true;
      for (const off of this.subscriptions.values()) off();
      this.subscriptions.clear();
      this.drainListeners.clear();
      socket.off('drain', this.emitDrain);
      handlers.onClose(this);
    });
  }

  private dispatchLine(line: string, handlers: ConnectionHandlers): void {
    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch (e) {
      this.sendParseError((e as Error).message);
      return;
    }
    handlers.onMessage(this, value);
  }

  private sendParseError(message: string): void {
    this.send({ jsonrpc: '2.0', id: null, error: { code: ErrorCode.Parse, message } });
  }

  /** 行の上限超過など、続行できない入力。Parse error を返して切る。 */
  private reject(message: string): void {
    this.rejecting = true;
    this.sendParseError(message);
    this.socket.destroySoon(); // 応答を送り切ってから destroy する (相手が閉じなくても残さない)
  }

  send(msg: RpcMessage): void {
    if (this.closed || this.socket.destroyed) return;
    if (this.socket.writableLength > MAX_WRITABLE_BYTES) {
      this.socket.destroy();
      return;
    }
    this.socket.write(encodeMessage(msg));
  }

  /** 購読ストリームが続きを送ってよいか。送信キューが STREAM_HIGH_WATER_BYTES 未満で、接続が生きているとき。 */
  isStreamWritable(): boolean {
    return !this.closed && !this.socket.destroyed && this.socket.writableLength < STREAM_HIGH_WATER_BYTES;
  }

  /** 送信キューが空になったときに fn を呼ぶ。戻り値で解除する (close でも解除される)。 */
  onDrain(fn: () => void): () => void {
    if (this.drainListeners.size === 0) this.socket.on('drain', this.emitDrain);
    this.drainListeners.add(fn);
    return () => {
      this.drainListeners.delete(fn);
      if (this.drainListeners.size === 0) this.socket.off('drain', this.emitDrain);
    };
  }

  private readonly emitDrain = (): void => {
    for (const fn of [...this.drainListeners]) fn();
  };

  notify(method: string, seq: number, ts: string, params: Record<string, unknown>): void {
    this.send({ jsonrpc: '2.0', method, params: { seq, ts, ...params } });
  }
}
