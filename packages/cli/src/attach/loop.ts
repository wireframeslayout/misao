import * as path from 'node:path';
import type { MisaoClient } from '@misao/sdk';
import type { PaneInfo } from '@misao/protocol';
import type { CliIo } from '../cli-io.js';
import type { KeyBindings } from '../config/index.js';
import { CliError } from '../errors.js';
import { loginShell, openTerminalPane } from '../open-pane.js';
import { writeLine } from '../output.js';
import { shortDisplayName } from '../view/pane-view.js';
import { attachablePanes, resumedNotice, runPicker } from './picker.js';
import { runSession } from './session.js';
import type { SessionEnd } from './session.js';

export interface AttachLoopOptions {
  client: MisaoClient;
  io: CliIo;
  keys: KeyBindings;
  /** 最初に入るペイン。 */
  initial: PaneInfo;
  isReadonly: boolean;
  replay: 'snapshot' | 'none';
}

/** 次にすること: 別のペインへ入る / 一覧を出す / 終了コードで終わる。 */
type Next = { kind: 'attach'; pane: PaneInfo } | { kind: 'list'; header: string[] } | { kind: 'exit'; code: number };

/** session と picker をつなぎ、ペイン間の移動 (n / p)、一覧 (l / d)、新しいシェル (一覧の n) を扱う。 */
export async function runAttachLoop(opts: AttachLoopOptions): Promise<number> {
  const { client, io } = opts;
  let next: Next = { kind: 'attach', pane: opts.initial };
  for (;;) {
    if (next.kind === 'exit') return next.code;
    if (next.kind === 'list') {
      const choice = await runPicker(io, await client.request('pane.list', {}), { header: next.header, canCreate: !opts.isReadonly });
      if (choice.kind === 'quit') return 0;
      next = { kind: 'attach', pane: choice.kind === 'new' ? await createShellPane(client, io) : choice.pane };
      continue;
    }
    const end = await attachOnce(opts, next.pane);
    next = await decideNext(opts, next.pane, end);
  }
}

async function createShellPane(client: MisaoClient, io: CliIo): Promise<PaneInfo> {
  const paneId = await openTerminalPane(client, { cmd: loginShell(io.shell), cwd: path.resolve(io.cwd), labels: {}, env: {}, windowId: undefined });
  return client.request('pane.info', { paneId });
}

/** 入る直前の状態を取り直し (blocked → working の知らせ)、終わっていれば入らない。 */
async function attachOnce(opts: AttachLoopOptions, listed: PaneInfo): Promise<SessionEnd> {
  const { client, io } = opts;
  const pane = await client.request('pane.info', { paneId: listed.paneId });
  const name = shortDisplayName(pane, io.homeDir);
  if (pane.processState !== 'running') throw new CliError('runtime', `${name} はすでに終了しています`);
  const resumed = resumedNotice(listed, pane, io.homeDir);
  if (resumed !== undefined) writeLine(io.stderr, `[misao] ${resumed}`);
  return runSession({ client, io, pane, keys: opts.keys, isReadonly: opts.isReadonly, replay: opts.replay });
}

async function decideNext(opts: AttachLoopOptions, current: PaneInfo, end: SessionEnd): Promise<Next> {
  const { client, io } = opts;
  const name = shortDisplayName(current, io.homeDir);
  switch (end.reason) {
    case 'failed':
      throw end.error;
    case 'disconnected':
      writeLine(io.stderr, '[misao] 接続が切れました');
      return { kind: 'exit', code: 1 };
    case 'signal':
      writeLine(io.stderr, '[misao] 終了シグナルを受けたため抜けました');
      return { kind: 'exit', code: 1 };
    case 'exited': {
      const how = end.exitCode === null ? `シグナル ${end.signal ?? '?'}` : `終了コード ${end.exitCode}`;
      const message = `[misao] ${name} が終了しました（${how}）`;
      if (attachablePanes(await client.request('pane.list', {})).length > 0) return { kind: 'list', header: [message] };
      writeLine(io.stdout, message);
      return { kind: 'exit', code: 0 };
    }
    case 'action': {
      if (end.action === 'list') return { kind: 'list', header: [] };
      if (end.action === 'detach') {
        return { kind: 'list', header: [`[misao] ${name} から抜けました（入力 ${end.inputCount} 回を記録。内容は保存していません）`] };
      }
      return neighbor(attachablePanes(await client.request('pane.list', {})), current, end.action === 'next' ? 1 : -1);
    }
  }
}

/** 一覧 (ls と同じ並び) で隣のペインへ。循環する。移動先が自分だけなら一覧へ戻る。 */
function neighbor(panes: readonly PaneInfo[], current: PaneInfo, step: 1 | -1): Next {
  const index = panes.findIndex((p) => p.paneId === current.paneId);
  const target = index === -1 ? panes[0] : panes[(index + step + panes.length) % panes.length];
  if (target === undefined || target.paneId === current.paneId) {
    return { kind: 'list', header: ['[misao] 移動できる別のペインがありません'] };
  }
  return { kind: 'attach', pane: target };
}
