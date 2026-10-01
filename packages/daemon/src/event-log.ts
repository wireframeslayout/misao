import { EventEmitter } from 'node:events';
import { nowIso } from './clock.js';
import { SeqRing } from './ring.js';

const EVENT_RING_CAP = 1000;

export interface DaemonEvent {
  type: string;
  data: Record<string, unknown>;
  /** pane に紐づくイベントのみ。data ではなく外側に載せる。 */
  paneId?: string;
}

/** デーモン全体のイベント: seq 付きリング + ライブ配信。 */
export class EventLog {
  readonly ring = new SeqRing<DaemonEvent>(EVENT_RING_CAP);
  private readonly bus = new EventEmitter();

  get head(): number {
    return this.ring.head;
  }

  emit(type: string, data: Record<string, unknown>, paneId?: string): void {
    const ts = nowIso();
    const event: DaemonEvent = paneId === undefined ? { type, data } : { type, data, paneId };
    const seq = this.ring.push(event, 1, ts);
    this.bus.emit('event', seq, event, ts);
  }

  on(cb: (seq: number, event: DaemonEvent, ts: string) => void): () => void {
    this.bus.on('event', cb);
    return () => this.bus.off('event', cb);
  }
}
