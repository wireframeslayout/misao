import type { EventParams, NotificationParams } from '@misao/protocol';

export type EventHandler = (event: EventParams) => void;
export type LineHandler = (line: NotificationParams<'pane.line'>) => void;

/** 購読できるストリームの識別子。 */
export type StreamId = { kind: 'events' } | { kind: 'lines'; paneId: string };

export interface EventsEntry {
  readonly kind: 'events';
  lastSeq: number;
  readonly handler: EventHandler;
}

export interface LinesEntry {
  readonly kind: 'lines';
  readonly paneId: string;
  lastSeq: number;
  readonly handler: LineHandler;
}

export type StreamEntry = EventsEntry | LinesEntry;

/** 利用側が永続化して、次回の since / epoch に渡せる位置。 */
export interface StreamPosition {
  seq: number;
  epoch: string;
}

export const EVENTS_KEY = 'events';

export function linesKey(paneId: string): string {
  return `lines:${paneId}`;
}

export function keyOf(stream: StreamId): string {
  return stream.kind === 'events' ? EVENTS_KEY : linesKey(stream.paneId);
}

/** エントリ (ハンドラと可変の lastSeq を持つ) から、利用側へ渡してよい識別子だけを取り出す。 */
export function toStreamId(entry: StreamEntry): StreamId {
  return entry.kind === 'events' ? { kind: 'events' } : { kind: 'lines', paneId: entry.paneId };
}

/**
 * ストリームごとの lastSeq と、デーモンの epoch を保持する。接続は持たない。
 * lastSeq は受信のたびに進むので、エントリは可変。
 */
export class StreamCursor {
  private readonly entries = new Map<string, StreamEntry>();
  private currentEpoch: string | undefined;

  get epoch(): string | undefined {
    return this.currentEpoch;
  }

  get(key: string): StreamEntry | undefined {
    return this.entries.get(key);
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  list(): StreamEntry[] {
    return [...this.entries.values()];
  }

  add(entry: StreamEntry): void {
    const key = keyOf(entry);
    if (this.entries.has(key)) throw new Error(`stream already registered: ${key}`);
    this.entries.set(key, entry);
  }

  /** 同じエントリが登録されているときだけ外す (置き換え済みの別エントリは消さない)。 */
  remove(entry: StreamEntry): void {
    const key = keyOf(entry);
    if (this.entries.get(key) === entry) this.entries.delete(key);
  }

  /**
   * epoch を記録する。前回と違えば全ストリームの lastSeq を 0 に戻し、戻したエントリを返す。
   * 初回 (前回なし) は何も戻さない。
   */
  resetForEpoch(epoch: string): StreamEntry[] {
    const previous = this.currentEpoch;
    this.currentEpoch = epoch;
    if (previous === undefined || previous === epoch) return [];
    const reset = this.list();
    for (const entry of reset) entry.lastSeq = 0;
    return reset;
  }

  /** 受信した seq を取り込む。lastSeq 以下 (重複・登録なし) なら false を返し、配信しない。 */
  accept(key: string, seq: number): boolean {
    const entry = this.entries.get(key);
    if (!entry || seq <= entry.lastSeq) return false;
    entry.lastSeq = seq;
    return true;
  }

  position(entry: StreamEntry): StreamPosition {
    return { seq: entry.lastSeq, epoch: this.requireEpoch() };
  }

  /** 再接続時の購読 params。 */
  resubscribeParams(entry: StreamEntry): { since: number; epoch: string } {
    return { since: entry.lastSeq, epoch: this.requireEpoch() };
  }

  private requireEpoch(): string {
    if (this.currentEpoch === undefined) throw new Error('epoch is not known before the first connection');
    return this.currentEpoch;
  }
}
