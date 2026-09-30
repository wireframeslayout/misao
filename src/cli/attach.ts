import { MisaoClient } from '../client/MisaoClient.js';

const PREFIX = 0x1d; // Ctrl-]

export interface AttachOptions {
  paneId: string;
  replay: 'raw' | 'snapshot' | 'none';
  readonly: boolean;
}

/** prefix キー (Ctrl-]) の状態機械。入力バイト列を「転送するバイト」と「detach 要求」に分ける。 */
export class PrefixFilter {
  private pending = false;

  feed(data: Buffer): { forward: Buffer; detach: boolean } {
    const out: number[] = [];
    for (const b of data) {
      if (this.pending) {
        this.pending = false;
        if (b === 0x64) return { forward: Buffer.from(out), detach: true }; // 'd'
        if (b === PREFIX) out.push(PREFIX);
        continue; // それ以外は無視
      }
      if (b === PREFIX) this.pending = true;
      else out.push(b);
    }
    return { forward: Buffer.from(out), detach: false };
  }
}

export async function attach(client: MisaoClient, opts: AttachOptions): Promise<number> {
  if (!process.stdin.isTTY) throw new Error('attach requires a TTY on stdin');
  const clientId = `cli-${process.pid}`;
  const { paneId } = opts;
  let exitReason = 'detached';
  let exitCode = 0;

  const done = new Promise<void>((resolve) => {
    client.on('close', () => {
      exitReason = 'connection closed';
      exitCode = 1;
      resolve();
    });
    client.onNotification((n) => {
      if (n.method === 'pane.output' && n.params.paneId === paneId) {
        process.stdout.write(Buffer.from(n.params.dataB64 as string, 'base64'));
      } else if (n.method === 'event' && n.params.type === 'pane.exited') {
        const data = n.params.data as { paneId: string; exitCode: number | null };
        if (data.paneId === paneId) {
          exitReason = `pane exited (code ${data.exitCode ?? 'signal'})`;
          resolve();
        }
      }
    });
    const filter = new PrefixFilter();
    process.stdin.on('data', (buf: Buffer) => {
      const { forward, detach } = filter.feed(buf);
      if (forward.length > 0 && !opts.readonly) {
        client
          .request('pane.write', { paneId, dataB64: forward.toString('base64'), source: 'terminal' })
          .catch(() => undefined);
      }
      if (detach) resolve();
    });
  });

  const sendResize = () =>
    client
      .request('pane.resize', {
        paneId,
        cols: process.stdout.columns || 80,
        rows: process.stdout.rows || 24,
        clientId,
      })
      .catch(() => undefined);

  process.stdin.setRawMode(true);
  process.stdin.resume();
  process.stderr.write(`[misao] attached to ${paneId} (Ctrl-] d to detach)\r\n`);
  try {
    await client.request('events.subscribe', {});
    if (!opts.readonly) await sendResize();
    process.on('SIGWINCH', sendResize);
    await client.request('pane.attach', { paneId, clientId, replay: opts.replay });
    await done;
    if (!client.isClosed) await client.request('pane.detach', { paneId }).catch(() => undefined);
  } finally {
    process.off('SIGWINCH', sendResize);
    process.stdin.setRawMode(false);
    process.stdin.pause();
    process.stderr.write(`\r\n[misao] ${exitReason}\r\n`);
  }
  return exitCode;
}
