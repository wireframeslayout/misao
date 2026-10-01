import { EventEmitter } from 'node:events';
import { nowIso } from './clock.js';
import { SeqRing } from './ring.js';

export interface DaemonEvent {
  type: string;
  data: Record<string, unknown>;
  /** pane に紐づくイベントのみ。data ではなく外側に載せる。 */
  paneId?: string;
}

/** デーモン全体のイベント: seq 付きリング + ライブ配信。 */
export class EventLog {
  readonly ring: SeqRing<DaemonEvent>;
  private readonly bus = new EventEmitter();

  constructor(cap: number) {
    this.ring = new SeqRing<DaemonEvent>(cap);
  }

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
