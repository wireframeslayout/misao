import * as net from 'node:net';
import { LineSplitter, RpcErrorResponseSchema, RpcSuccessResponseSchema, encodeMessage } from '@misao/protocol';
import type { RpcNotification } from '@misao/protocol';

export class RpcClientError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

/** テスト専用の最小 NDJSON クライアント。 */
export class RpcClient {
  private readonly splitter = new LineSplitter();
  private readonly pending = new Map<number, Pending>();
  private readonly listeners: Array<(n: RpcNotification) => void> = [];
  private nextId = 1;

  private constructor(private readonly socket: net.Socket) {
    socket.on('data', (chunk: Buffer) => {
      for (const line of this.splitter.push(chunk)) this.onLine(line);
    });
    socket.on('error', () => undefined);
  }

  static async connect(socketPath: string): Promise<RpcClient> {
    const socket = net.connect(socketPath);
    await new Promise<void>((resolve, reject) => {
      socket.once('connect', resolve);
      socket.once('error', reject);
    });
    return new RpcClient(socket);
  }

  private onLine(line: string): void {
    const msg: unknown = JSON.parse(line);
    const ok = RpcSuccessResponseSchema.safeParse(msg);
    const err = RpcErrorResponseSchema.safeParse(msg);
    if (ok.success && typeof ok.data.id === 'number') {
      this.settle(ok.data.id, (p) => p.resolve(ok.data.result));
    } else if (err.success && typeof err.data.id === 'number') {
      this.settle(err.data.id, (p) => p.reject(new RpcClientError(err.data.error.code, err.data.error.message)));
    } else {
      for (const fn of this.listeners) fn(msg as RpcNotification);
    }
  }

  private settle(id: number, fn: (p: Pending) => void): void {
    const p = this.pending.get(id);
    if (!p) return;
    this.pending.delete(id);
    fn(p);
  }

  request<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: (v) => resolve(v as T), reject });
      this.socket.write(encodeMessage({ jsonrpc: '2.0', id, method, ...(params ? { params } : {}) }));
    });
  }

  onNotification(fn: (n: RpcNotification) => void): void {
    this.listeners.push(fn);
  }

  close(): void {
    this.socket.destroy();
  }
}
