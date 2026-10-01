import type { MisaoClient, Subscription } from '@misao/sdk';
import { parseKnownEvent } from '@misao/protocol';
import type { PaneInfo } from '@misao/protocol';
import type { CliIo } from '../cli-io.js';
import { formatKeySpec } from '../config/index.js';
import type { KeyBindings } from '../config/index.js';
import { writeLine } from '../output.js';
import { displayName, foregroundCommand, paneStateKey, taskNumber } from '../view/pane-view.js';
import { PrefixFilter } from './prefix-filter.js';
import type { PrefixAction } from './prefix-filter.js';
import { TtyGuard, terminalSize } from './tty.js';

export interface SessionOptions {
  client: MisaoClient;
  io: CliIo;
  pane: PaneInfo;
  keys: KeyBindings;
  /** 入力を送らず、resize もしない。 */
  isReadonly: boolean;
  replay: 'snapshot' | 'none';
}

type SessionReason =
  | { reason: 'action'; action: PrefixAction }
  | { reason: 'exited'; exitCode: number | null; signal: number | null }
  | { reason: 'disconnected' }
  | { reason: 'signal' }
  /** 入力の送信や resize がデーモンに拒否された。 */
  | { reason: 'failed'; error: unknown };

/** inputCount は、この attach で CLI が送った pane.write の回数。 */
export type SessionEnd = SessionReason & { inputCount: number };

/** `[misao] W-806 · misao 計画 · task #419 · claude · blocked · 抜ける: Ctrl-^ d · 次/前: Ctrl-^ n/p · 一覧: Ctrl-^ l` */
export function formatBanner(pane: PaneInfo, keys: KeyBindings, isReadonly: boolean, homeDir: string): string {
  const prefix = formatKeySpec(keys.prefix);
  const task = taskNumber(pane);
  const parts = [
    displayName(pane, homeDir),
    ...(task === undefined ? [] : [`task #${task}`]),
    foregroundCommand(pane),
    paneStateKey(pane),
    ...(isReadonly ? ['READONLY'] : []),
    `抜ける: ${prefix} ${formatKeySpec(keys.detach)}`,
    `次/前: ${prefix} ${formatKeySpec(keys.next)}/${formatKeySpec(keys.prev)}`,
    `一覧: ${prefix} ${formatKeySpec(keys.list)}`,
  ];
  return `[misao] ${parts.join(' · ')}`;
}

/**
 * 1 つのペインへ attach して、終わるまで入出力をつなぐ。どの終わり方 (prefix アクション / ペイン終了 /
 * 接続断 / シグナル / 例外) でも、端末の raw mode を戻して TTY_RESET を書いてから返す (または投げる)。
 */
export async function runSession(opts: SessionOptions): Promise<SessionEnd> {
  const { client, io, pane, keys, isReadonly } = opts;
  const { paneId } = pane;
  const clientId = `cli-${io.pid}`;
  const size = terminalSize(io);
  const guard = new TtyGuard(io);
  const filter = new PrefixFilter(keys);
  let inputCount = 0;
  let finish: (reason: SessionReason) => void = () => undefined;
  const done = new Promise<SessionReason>((resolve) => (finish = resolve));
  const fail = (promise: Promise<unknown>): void => void promise.catch((error: unknown) => finish({ reason: 'failed', error }));
  const cleanups: Array<() => void> = [];
  let events: Subscription | undefined;
  let isAttached = false;
  let isDisconnected = false;

  writeLine(io.stderr, formatBanner(pane, keys, isReadonly, io.homeDir));
  guard.enter(() => finish({ reason: 'signal' }));
  try {
    cleanups.push(
      client.onNotification((n) => {
        if (n.method === 'pane.output' && n.params.paneId === paneId) io.stdout.write(Buffer.from(n.params.dataB64, 'base64'));
      }),
      client.onStateChange((state) => {
        if (state.status === 'connected') return;
        isDisconnected = true;
        finish({ reason: 'disconnected' });
      }),
    );
    const onInput = (chunk: Buffer | string): void => {
      const { forward, action } = filter.feed(Buffer.from(chunk));
      if (forward.length > 0 && !isReadonly) {
        inputCount++;
        fail(client.request('pane.write', { paneId, dataB64: forward.toString('base64'), source: 'terminal', clientId }));
      }
      if (action !== null) finish({ reason: 'action', action });
    };
    io.stdin.on('data', onInput);
    cleanups.push(() => void io.stdin.off('data', onInput));
    if (!isReadonly) {
      cleanups.push(
        io.onSignal('SIGWINCH', () => {
          const { cols, rows } = terminalSize(io);
          fail(client.request('pane.resize', { paneId, cols, rows, clientId }));
        }),
      );
    }
    events = await client.subscribeEvents((event) => {
      const known = parseKnownEvent(event);
      if (known?.type === 'pane.exited' && known.paneId === paneId) {
        finish({ reason: 'exited', exitCode: known.data.exitCode, signal: known.data.signal });
      }
    });
    await client.request('pane.attach', {
      paneId,
      clientId,
      replay: opts.replay,
      ...(isReadonly ? {} : { cols: size.cols, rows: size.rows }),
    });
    isAttached = true;
    // attach の前に終わっていたペインは終了イベントが来ないので、状態を見て終わらせる。
    const current = await client.request('pane.info', { paneId });
    if (current.processState !== 'running') finish({ reason: 'exited', exitCode: current.exitCode, signal: current.signal });
    return { ...(await done), inputCount };
  } finally {
    for (const cleanup of cleanups) cleanup();
    events?.unsubscribe();
    guard.restore();
    if (isAttached && !isDisconnected) await client.request('pane.detach', { paneId });
  }
}
