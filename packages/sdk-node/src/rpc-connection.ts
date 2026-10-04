import * as net from 'node:net';
import type { Duplex } from 'node:stream';
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
import { readHotNotification } from './hot-notification.js';
import { Listeners } from './listeners.js';

export type Notification = {
  [N in NotificationName]: { method: N; params: NotificationParams<N> };
}[NotificationName];

/** connect オプションに渡す関数。signal は試行の打ち切り (タイムアウト・close()) で abort される。 */
export type ConnectFunction = (options: { signal: AbortSignal }) => Promise<Duplex>;

export interface ConnectAttempt {
  controller: AbortController;
  timeoutMs: number;
}

export type Settled<T> = { ok: true; value: T } | { ok: false; error: Error };

function isNotificationName(name: string): name is NotificationName {
  return Object.hasOwn(notifications, name);
}

/**
 * 1 本のソケット (または差し込まれた Duplex) 上の JSON-RPC。再接続はしない (上位の MisaoClient が新しい接続を作る)。
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
    private readonly socket: Duplex,
    report: (error: unknown) => void,
  ) {
    this.closeListeners = new Listeners(report);
    this.notificationListeners = new Listeners(report);
    socket.on('data', this.onSocketData);
    socket.on('error', this.onSocketError);
    // net.Socket 以外の Duplex は相手の end で自動的に閉じるとは限らない。プロトコルは半閉じを使わないので閉じる。
    socket.on('end', this.onSocketEnd);
    socket.on('close', this.onSocketClose);
  }

  private readonly onSocketData = (chunk: Buffer): void => this.onData(chunk);
  private readonly onSocketError = (cause: Error): void =>
    this.fail(new MisaoConnectionError(cause.message, { cause }));
  private readonly onSocketEnd = (): void => {
    this.socket.destroy();
  };

  /**
   * target: Unix ソケットのパス、または接続済みの Duplex を返す関数 (WebSocket 中継など)。
   * 関数の場合は attempt.timeoutMs で打ち切り、attempt.controller の abort でも中止する。
   * どちらでも打ち切った時点で signal を abort し、あとから解決した Duplex は破棄する。
   * report: リスナーが投げた例外の報告先。
   */
  static connect(target: string, report: (error: unknown) => void): Promise<RpcConnection>;
  static connect(target: string | ConnectFunction, report: (error: unknown) => void, attempt: ConnectAttempt): Promise<RpcConnection>;
  static connect(target: string | ConnectFunction, report: (error: unknown) => void, attempt?: ConnectAttempt): Promise<RpcConnection> {
    if (typeof target === 'string') return RpcConnection.connectSocket(target, report);
    if (!attempt) throw new TypeError('a connect function requires an attempt (controller and timeoutMs)');
    return RpcConnection.connectCustom(target, report, attempt);
  }

  private static connectCustom(
    target: ConnectFunction,
    report: (error: unknown) => void,
    { controller, timeoutMs }: ConnectAttempt,
  ): Promise<RpcConnection> {
    const { signal } = controller;
    return new Promise((resolve, reject) => {
      let settled = false;
      const settle = (): boolean => {
        if (settled) return false;
        settled = true;
        clearTimeout(timer);
        signal.removeEventListener('abort', onAbort);
        return true;
      };
      const failWith = (message: string, cause?: unknown): void => {
        if (settle()) reject(new MisaoConnectionError(`cannot connect: ${message}`, { cause }));
      };
      const onAbort = (): void => failWith('aborted');
      const timer = setTimeout(() => {
        failWith(`timed out after ${timeoutMs}ms`);
        controller.abort();
      }, timeoutMs);
      signal.addEventListener('abort', onAbort, { once: true });
      if (signal.aborted) {
        onAbort();
        return;
      }
      Promise.resolve()
        .then(() => target({ signal }))
        .then(
          (stream) => {
            if (settled) {
              stream.destroy(); // 打ち切り済み。あとから来た接続は誰も使わない。
              return;
            }
            settle();
            if (stream.destroyed) {
              reject(new MisaoConnectionError('cannot connect: the stream returned by connect() is already destroyed'));
              return;
            }
            resolve(new RpcConnection(stream, report));
          },
          (cause: unknown) => failWith(cause instanceof Error ? cause.message : String(cause), cause),
        );
    });
  }

  private static connectSocket(socketPath: string, report: (error: unknown) => void): Promise<RpcConnection> {
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
    const hot = readHotNotification(json);
    if (hot) {
      if (hot.ok) this.notificationListeners.emit(hot.notification);
      else this.fail(new MisaoConnectionError(`protocol violation: ${hot.message}`));
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

  private readonly onSocketClose = (): void => {
    this.closed = true;
    // 外から Duplex を持ち続けられても、この接続 (とクライアント) を参照させない。
    this.socket.off('data', this.onSocketData);
    this.socket.off('end', this.onSocketEnd);
    this.socket.off('close', this.onSocketClose);
    this.socket.off('error', this.onSocketError);
    this.socket.on('error', () => undefined); // 閉じたあとの error で uncaught にしない
    const reason = this.failure ?? new MisaoConnectionError('connection closed by peer');
    const settles = [...this.pending.values()];
    this.pending.clear();
    for (const settle of settles) settle({ ok: false, error: reason });
    this.closeListeners.emit(reason);
  };
}
