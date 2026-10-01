import * as net from 'node:net';
import {
  LineSplitter,
  RpcMessageSchema,
  encodeMessage,
  isNotification,
  isResponse,
  methods,
  notifications,
} from '@misao/protocol';
import type {
  MethodName,
  MethodParams,
  MethodResult,
  NotificationName,
  NotificationParams,
  RpcNotification,
  RpcResponse,
} from '@misao/protocol';
import { MisaoConnectionError, MisaoRpcError } from './errors.js';
import { Listeners } from './listeners.js';

export type Notification = {
  [N in NotificationName]: { method: N; params: NotificationParams<N> };
}[NotificationName];

export type Settled<T> = { ok: true; value: T } | { ok: false; error: Error };

function isNotificationName(name: string): name is NotificationName {
  return Object.hasOwn(notifications, name);
}

/**
 * 1 本のソケット上の JSON-RPC。再接続はしない (上位の MisaoClient が新しい接続を作る)。
 * 行上限超過・不正 JSON・スキーマ違反はプロトコル違反として接続を切り、保留中の要求を reject する。
 */
export class RpcConnection {
  private readonly splitter = new LineSplitter();
  private readonly pending = new Map<number, (outcome: Settled<unknown>) => void>();
  private readonly closeListeners: Listeners<MisaoConnectionError>;
  private readonly notificationListeners: Listeners<Notification>;
  private nextId = 1;
  private closed = false;
  private failure: MisaoConnectionError | undefined;

  private constructor(
    private readonly socket: net.Socket,
    report: (error: unknown) => void,
  ) {
    this.closeListeners = new Listeners(report);
    this.notificationListeners = new Listeners(report);
    socket.on('data', (chunk: Buffer) => this.onData(chunk));
    socket.on('error', (cause) => this.fail(new MisaoConnectionError(cause.message, { cause })));
    socket.on('close', () => this.onSocketClose());
  }

  /** report: リスナーが投げた例外の報告先。 */
  static connect(socketPath: string, report: (error: unknown) => void): Promise<RpcConnection> {
    return new Promise((resolve, reject) => {
      const socket = net.connect(socketPath);
      const onError = (cause: Error): void =>
        reject(new MisaoConnectionError(`cannot connect to ${socketPath}: ${cause.message}`, { cause }));
      socket.once('error', onError);
      socket.once('connect', () => {
        socket.off('error', onError);
        resolve(new RpcConnection(socket, report));
      });
    });
  }

  get isClosed(): boolean {
    return this.closed || this.failure !== undefined;
  }

  onClose(callback: (reason: MisaoConnectionError) => void): () => void {
    return this.closeListeners.add(callback);
  }

  onNotification(callback: (notification: Notification) => void): () => void {
    return this.notificationListeners.add(callback);
  }

  request<M extends MethodName>(method: M, params: MethodParams<M>): Promise<MethodResult<M>> {
    return new Promise((resolve, reject) => {
      this.requestSync(method, params, (outcome) => (outcome.ok ? resolve(outcome.value) : reject(outcome.error)));
    });
  }

  /**
   * 応答行を処理したその場で onSettled を同期的に呼ぶ。同じチャンクで続く再生通知より先に
   * 購読結果を反映するために使う (Promise 経由だとチャンク内の後続行の処理が先に走る)。
   * 送信前の params 検証失敗と未接続は同期的に throw する。
   */
  requestSync<M extends MethodName>(
    method: M,
    params: MethodParams<M>,
    onSettled: (outcome: Settled<MethodResult<M>>) => void,
  ): void {
    if (this.isClosed) throw new MisaoConnectionError('not connected');
    const definition = methods[method];
    // methods の params はすべて z.object なので、検証後の値は Record になる。
    const validated = definition.params.parse(params) as Record<string, unknown>;
    const id = this.nextId++;
    this.pending.set(id, (outcome) => {
      if (!outcome.ok) {
        onSettled(outcome);
        return;
      }
      const parsed = definition.result.safeParse(outcome.value);
      if (parsed.success) {
        onSettled({ ok: true, value: parsed.data as MethodResult<M> });
        return;
      }
      const error = new MisaoConnectionError(`protocol violation: invalid result for ${method}: ${parsed.error.message}`);
      onSettled({ ok: false, error });
      this.fail(error);
    });
    this.socket.write(encodeMessage({ jsonrpc: '2.0', id, method, params: validated }));
  }

  close(): void {
    this.fail(new MisaoConnectionError('connection closed by client'));
  }

  private onData(chunk: Buffer): void {
    let lines: string[];
    try {
      lines = this.splitter.push(chunk);
    } catch (cause) {
      this.fail(new MisaoConnectionError(`protocol violation: ${(cause as Error).message}`, { cause }));
      return;
    }
    for (const line of lines) {
      if (this.failure) return;
      this.handleLine(line);
    }
  }

  private handleLine(line: string): void {
    let json: unknown;
    try {
      json = JSON.parse(line);
    } catch (cause) {
      this.fail(new MisaoConnectionError('protocol violation: invalid JSON line', { cause }));
      return;
    }
    const message = RpcMessageSchema.safeParse(json);
    if (!message.success) {
      this.fail(new MisaoConnectionError(`protocol violation: ${message.error.message}`));
    } else if (isResponse(message.data)) {
      this.handleResponse(message.data);
    } else if (isNotification(message.data)) {
      this.handleNotification(message.data);
    } else {
      this.fail(new MisaoConnectionError('protocol violation: unexpected request from server'));
    }
  }

  private handleResponse(response: RpcResponse): void {
    const settle = typeof response.id === 'number' ? this.pending.get(response.id) : undefined;
    if (!settle || typeof response.id !== 'number') {
      this.fail(new MisaoConnectionError(`protocol violation: response for unknown id ${String(response.id)}`));
      return;
    }
    this.pending.delete(response.id);
    if (response.error !== undefined) {
      const { code, message, data } = response.error;
      settle({ ok: false, error: new MisaoRpcError(code, message, data) });
    } else {
      settle({ ok: true, value: response.result });
    }
  }

  private handleNotification(notification: RpcNotification): void {
    // 未知の通知名はプロトコルの minor 追加として読み捨てる (既知の名前の params 不正は違反)。
    if (!isNotificationName(notification.method)) return;
    const parsed = notifications[notification.method].safeParse(notification.params);
    if (!parsed.success) {
      this.fail(
        new MisaoConnectionError(`protocol violation: invalid ${notification.method} params: ${parsed.error.message}`),
      );
      return;
    }
    this.notificationListeners.emit({ method: notification.method, params: parsed.data } as Notification);
  }

  private fail(error: MisaoConnectionError): void {
    if (this.failure) return;
    this.failure = error;
    this.socket.destroy();
  }

  private onSocketClose(): void {
    this.closed = true;
    const reason = this.failure ?? new MisaoConnectionError('connection closed by peer');
    const settles = [...this.pending.values()];
    this.pending.clear();
    for (const settle of settles) settle({ ok: false, error: reason });
    this.closeListeners.emit(reason);
  }
}
