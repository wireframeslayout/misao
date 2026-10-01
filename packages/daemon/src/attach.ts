import { nowIso } from './clock.js';
import type { Pane } from './pane.js';
import type { RequestContext } from './stream.js';

export type AttachReplay = 'raw' | 'snapshot' | 'none';

export interface AttachResult {
  head: number;
  oldest: number;
  truncated: boolean;
}

/**
 * pane の raw 出力を接続へ流す。replay を送ってからライブに切り替える (境界は head)。
 * 購読と clients への登録は最初の await より前に同期的に行う。
 * 応答前に同じ接続で再 attach / detach されたら、この attach の再生とキューは捨てる。
 */
export async function attachPane(
  ctx: RequestContext,
  pane: Pane,
  clientId: string,
  replay: AttachReplay,
): Promise<AttachResult> {
  const { conn } = ctx;
  const queue: Array<[number, Buffer, string]> = [];
  let live = false;
  const send = (seq: number, data: Buffer, ts: string): void =>
    conn.notify('pane.output', seq, ts, { paneId: pane.id, dataB64: data.toString('base64') });
  const listener = (seq: number, data: Buffer, ts: string): void => {
    if (live) send(seq, data, ts);
    else queue.push([seq, data, ts]);
  };
  pane.on('output', listener);
  const attachment = { clientId, off: () => pane.off('output', listener) };
  conn.attachments.set(pane.id, attachment);
  pane.clients.add(clientId);

  let head = pane.rawRing.head;
  let snapshot: string | undefined;
  if (replay === 'snapshot') {
    const snap = await pane.snapshot();
    head = snap.headSeq;
    snapshot = snap.data;
  }
  ctx.afterReply(() => {
    if (conn.attachments.get(pane.id) !== attachment) return; // 置き換え済み / detach 済み
    if (replay === 'raw') {
      for (const e of pane.rawRing.since(0)) if (e.seq <= head) send(e.seq, e.item, e.ts);
    } else if (snapshot !== undefined) {
      conn.notify('pane.output', head, nowIso(), {
        paneId: pane.id,
        dataB64: Buffer.from(snapshot, 'utf8').toString('base64'),
        replay: 'snapshot',
      });
    }
    for (const [seq, data, ts] of queue) if (seq > head) send(seq, data, ts);
    queue.length = 0;
    live = true;
  });
  return { head, oldest: pane.rawRing.oldest, truncated: replay === 'raw' && pane.rawRing.hasGap(0) };
}
