import { PROTOCOL_VERSION, isCompatibleProtocolVersion } from '@misao/protocol';
import type { MethodName, MethodParams, MethodResult } from '@misao/protocol';
import { DEFAULT_BACKOFF, computeBackoffDelay } from './backoff.js';
import type { BackoffOptions } from './backoff.js';
import { MisaoConnectionError, MisaoProtocolVersionError } from './errors.js';
import { Listeners } from './listeners.js';
import { RpcConnection } from './rpc-connection.js';
import type { Notification } from './rpc-connection.js';
import type { EventHandler, LineHandler } from './stream-cursor.js';
import { StreamSubscriber } from './stream-subscriber.js';
import type { GapInfo, SubscribeOptions, Subscription, SubscriptionErrorInfo } from './stream-subscriber.js';

export interface MisaoClientOptions {
  socketPath: string;
  backoff?: Partial<BackoffOptions>;
}

export type ConnectionState =
  | { status: 'connected' }
  | { status: 'reconnecting'; attempt: number; delayMs: number; cause: Error }
  | { status: 'closed' };

type Phase = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'closed';

/**
 * デーモンへの Node クライアント。切断したら指数バックオフで再接続し、登録済みのストリームを
 * 最後の seq (since) から自動で再購読する。デーモンの epoch が変わったら since を捨てて gap を通知する。
 *
 * pane.attach は since を持たないので自動復元しない。利用側が onStateChange の 'connected' を見て attach し直す。
 */
export class MisaoClient {
  private readonly socketPath: string;
  private readonly backoff: BackoffOptions;
  private readonly subscriber = new StreamSubscriber();
  private readonly stateListeners = new Listeners<ConnectionState>();
  private readonly notificationListeners = new Listeners<Notification>();
  private phase: Phase = 'idle';
  private conn: RpcConnection | undefined;
  private cancelSleep: (() => void) | undefined;

  constructor({ socketPath, backoff }: MisaoClientOptions) {
    this.socketPath = socketPath;
    this.backoff = { ...DEFAULT_BACKOFF, ...backoff };
  }

  /** 初回接続。失敗とプロトコル非互換は reject する。つながった後は close() まで再接続し続ける。 */
  async connect(): Promise<void> {
    if (this.phase !== 'idle') throw new Error(`connect() is not allowed while ${this.phase}`);
    this.phase = 'connecting';
    try {
      this.conn = await this.establish();
    } catch (error) {
      if (!this.isClosed()) this.phase = 'idle'; // close() が割り込んでいたら closed のまま
      throw error;
    }
    this.phase = 'connected';
    this.stateListeners.emit({ status: 'connected' });
  }

  async request<M extends MethodName>(method: M, params: MethodParams<M>): Promise<MethodResult<M>> {
    return this.requireConnection().request(method, params);
  }

  async subscribeEvents(handler: EventHandler, options: SubscribeOptions = {}): Promise<Subscription> {
    return this.subscriber.subscribeEvents(this.requireConnection(), handler, options);
  }

  async subscribeLines(paneId: string, handler: LineHandler, options: SubscribeOptions = {}): Promise<Subscription> {
    return this.subscriber.subscribeLines(this.requireConnection(), paneId, handler, options);
  }

  onStateChange(callback: (state: ConnectionState) => void): () => void {
    return this.stateListeners.add(callback);
  }

  /** 取りこぼしの通知。利用側は画面の再取得などで復旧する。 */
  onGap(callback: (gap: GapInfo) => void): () => void {
    return this.subscriber.onGap(callback);
  }

  /** 再購読がデーモンに拒否された (例: pane が消えた)。そのストリームは外れている。 */
  onSubscriptionError(callback: (info: SubscriptionErrorInfo) => void): () => void {
    return this.subscriber.onSubscriptionError(callback);
  }

  /** 購読の対象外の通知 (pane.output)。 */
  onNotification(callback: (notification: Notification) => void): () => void {
    return this.notificationListeners.add(callback);
  }

  close(): void {
    if (this.phase === 'closed') return;
    this.phase = 'closed';
    this.cancelSleep?.();
    this.conn?.close();
    this.conn = undefined;
    this.stateListeners.emit({ status: 'closed' });
  }

  private isClosed(): boolean {
    return this.phase === 'closed';
  }

  private requireConnection(): RpcConnection {
    if (!this.conn) throw new MisaoConnectionError('not connected');
    return this.conn;
  }

  /** 接続して server.info を確認し、ストリームを復元する。失敗したら接続を閉じて throw する。 */
  private async establish(): Promise<RpcConnection> {
    const conn = await RpcConnection.connect(this.socketPath);
    conn.onNotification((notification) => {
      if (!this.subscriber.dispatch(notification)) this.notificationListeners.emit(notification);
    });
    conn.onClose((reason) => this.handleDisconnect(conn, reason));
    try {
      const info = await conn.request('server.info', {});
      if (!isCompatibleProtocolVersion(info.protocolVersion, PROTOCOL_VERSION)) {
        throw new MisaoProtocolVersionError(info.protocolVersion, PROTOCOL_VERSION);
      }
      await this.subscriber.restore(conn, info.epoch);
      if (this.isClosed() || conn.isClosed) throw new MisaoConnectionError('connection closed during setup');
    } catch (error) {
      conn.close();
      throw error;
    }
    return conn;
  }

  private handleDisconnect(conn: RpcConnection, reason: MisaoConnectionError): void {
    if (conn !== this.conn) return;
    this.conn = undefined;
    this.phase = 'reconnecting';
    void this.reconnect(reason);
  }

  private async reconnect(initialCause: Error): Promise<void> {
    let cause = initialCause;
    for (let attempt = 1; this.phase === 'reconnecting'; attempt++) {
      const delayMs = computeBackoffDelay(attempt, this.backoff);
      this.stateListeners.emit({ status: 'reconnecting', attempt, delayMs, cause });
      await this.sleep(delayMs);
      if (this.phase !== 'reconnecting') return;
      try {
        this.conn = await this.establish();
      } catch (error) {
        cause = error as Error;
        continue;
      }
      this.phase = 'connected';
      this.stateListeners.emit({ status: 'connected' });
      return;
    }
  }

  /** close() で即座に解ける待機。 */
  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const wake = (): void => {
        clearTimeout(timer);
        this.cancelSleep = undefined;
        resolve();
      };
      const timer = setTimeout(wake, ms);
      this.cancelSleep = wake;
    });
  }
}
