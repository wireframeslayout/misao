import { EventEmitter } from 'node:events';
import * as net from 'node:net';
import {
  LineSplitter,
  encodeMessage,
  isNotification,
  isResponse,
  type RpcMessage,
  type RpcNotification,
} from '../protocol.js';

export class RpcRemoteError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

/**
 * 'notification' (RpcNotification) / 'close' を emit する。
 * request は id → Promise の対応表で解決する。接続断で未解決 request は reject。
 */
export class MisaoClient extends EventEmitter {
  private nextId = 1;
  private readonly pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private readonly splitter = new LineSplitter();
  private closed = false;

  private constructor(private readonly socket: net.Socket) {
    super();
    socket.on('data', (chunk) => {
      for (const line of this.splitter.push(chunk)) this.handle(line);
    });
    socket.on('error', () => socket.destroy());
    socket.on('close', () => {
      this.closed = true;
      for (const p of this.pending.values()) p.reject(new Error('connection closed'));
      this.pending.clear();
      this.emit('close');
    });
  }

  static connect(socketPath: string): Promise<MisaoClient> {
    return new Promise((resolve, reject) => {
      const socket = net.connect(socketPath);
      socket.once('connect', () => {
        socket.removeListener('error', reject);
        resolve(new MisaoClient(socket));
      });
      socket.once('error', reject);
    });
  }

  private handle(line: string): void {
    let msg: RpcMessage;
    try {
      msg = JSON.parse(line) as RpcMessage;
    } catch {
      return;
    }
    if (isResponse(msg)) {
      const p = this.pending.get(msg.id as number);
      if (!p) return;
      this.pending.delete(msg.id as number);
      if (msg.error) p.reject(new RpcRemoteError(msg.error.code, msg.error.message));
      else p.resolve(msg.result);
    } else if (isNotification(msg)) {
      this.emit('notification', msg satisfies RpcNotification);
    }
  }

  request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (this.closed) return Promise.reject(new Error('connection closed'));
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.socket.write(encodeMessage({ jsonrpc: '2.0', id, method, params }));
    });
  }

  onNotification(handler: (n: RpcNotification) => void): void {
    this.on('notification', handler);
  }

  get isClosed(): boolean {
    return this.closed;
  }

  close(): void {
    this.socket.end();
    this.socket.destroy();
  }
}
