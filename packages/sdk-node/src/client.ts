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
  /** cause は再接続先がプロトコル非互換で打ち切ったとき。close() による終了では無い。 */
  | { status: 'closed'; cause?: MisaoProtocolVersionError };

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
  /** 要求・購読に使う接続。server.info の互換確認が済んだ時点で入る (ストリーム復元中も使える)。 */
  private conn: RpcConnection | undefined;
  /** 確立途中の接続。close() で確実に閉じるために持つ。 */
  private establishing: RpcConnection | undefined;
  private cancelSleep: (() => void) | undefined;

  constructor({ socketPath, backoff }: MisaoClientOptions) {
    this.socketPath = socketPath;
    this.backoff = { ...DEFAULT_BACKOFF, ...backoff };
  }

  /**
   * 初回接続。失敗とプロトコル非互換は reject する。つながった後は close() まで再接続し続ける
   * (再接続先がプロトコル非互換なら打ち切って closed になる)。
   */
  async connect(): Promise<void> {
    if (this.phase !== 'idle') throw new Error(`connect() is not allowed while ${this.phase}`);
    this.phase = 'connecting';
    try {
      await this.establish();
    } catch (error) {
      if (!this.isClosed()) this.phase = 'idle'; // close() が割り込んでいたら closed のまま
      throw error;
    }
    if (this.isClosed()) throw new MisaoConnectionError('client closed during connect');
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
    this.shutdown({ status: 'closed' });
  }

  private shutdown(state: ConnectionState & { status: 'closed' }): void {
    if (this.phase === 'closed') return;
    this.phase = 'closed';
    this.cancelSleep?.();
    this.establishing?.close();
    this.conn?.close();
    this.establishing = undefined;
    this.conn = undefined;
    this.stateListeners.emit(state);
  }

  private isClosed(): boolean {
    return this.phase === 'closed';
  }

  private requireConnection(): RpcConnection {
    if (!this.conn) throw new MisaoConnectionError('not connected');
    return this.conn;
  }

  /**
   * 接続して server.info を確認し、ストリームを復元する。互換確認の後は this.conn に置くので、
   * 復元中に出る gap の通知から request() で再取得できる。失敗したら接続を閉じて throw する。
   */
  private async establish(): Promise<void> {
    const conn = await RpcConnection.connect(this.socketPath);
    if (this.isClosed()) {
      conn.close();
      throw new MisaoConnectionError('client closed during connect');
    }
    this.establishing = conn;
    conn.onNotification((notification) => {
      if (!this.subscriber.dispatch(notification)) this.notificationListeners.emit(notification);
    });
    conn.onClose((reason) => this.handleDisconnect(conn, reason));
    try {
      const info = await conn.request('server.info', {});
      if (!isCompatibleProtocolVersion(info.protocolVersion, PROTOCOL_VERSION)) {
        throw new MisaoProtocolVersionError(info.protocolVersion, PROTOCOL_VERSION);
      }
      this.conn = conn;
      await this.subscriber.restore(conn, info.epoch);
      if (conn.isClosed) throw new MisaoConnectionError('connection closed during setup');
    } catch (error) {
      if (this.conn === conn) this.conn = undefined;
      conn.close();
      throw error;
    } finally {
      if (this.establishing === conn) this.establishing = undefined;
    }
  }

  /** 確立済みの接続が切れたときだけ再接続を始める。確立途中の切断は establish が throw して扱う。 */
  private handleDisconnect(conn: RpcConnection, reason: MisaoConnectionError): void {
    if (conn !== this.conn) return;
    this.conn = undefined;
    if (this.phase !== 'connected') return;
    this.phase = 'reconnecting';
    void this.reconnect(reason);
  }

  private async reconnect(initialCause: Error): Promise<void> {
    let cause = initialCause;
    for (let attempt = 1; this.phase === 'reconnecting'; attempt++) {
      const delayMs = computeBackoffDelay(attempt, this.backoff);
      this.stateListeners.emit({ status: 'reconnecting', attempt, delayMs, cause });
      if (this.phase !== 'reconnecting') return; // リスナーが close() した
      await this.sleep(delayMs);
      if (this.phase !== 'reconnecting') return;
      try {
        await this.establish();
      } catch (error) {
        // 非互換は待っても直らない。打ち切って利用側へ知らせる (fail fast)。
        if (error instanceof MisaoProtocolVersionError) {
          this.shutdown({ status: 'closed', cause: error });
          return;
        }
        cause = error as Error;
        continue;
      }
      if (this.phase !== 'reconnecting') return; // 確立直後に close() された
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
