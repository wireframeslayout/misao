import type { MisaoClient, Subscription } from '@misao/sdk';
import type { CliIo } from './cli-io.js';
import { CliError } from './errors.js';
import { writeLine } from './output.js';

export interface FollowOptions {
  /** 最初の購読で、保持範囲外 (truncated) の gap を警告するか。--since を指定したときだけ true。 */
  warnTruncated: boolean;
  /** 購読を始める時点のデーモンの epoch (server.info)。 */
  initialEpoch: string;
  /** epochOf は、いま流れている seq が属する epoch を返す (--json の各行と警告に載せる)。 */
  subscribe(client: MisaoClient, epochOf: () => string): Promise<Subscription>;
}

export interface StartPosition {
  since: number;
  epoch: string;
}

/**
 * --since / --epoch から購読の開始位置を決める。--epoch は --since と組でだけ使える。
 * --since だけなら現在の epoch の seq とみなし、その旨を stderr に警告する。
 */
export function resolveStartPosition(
  io: CliIo,
  since: number | undefined,
  epoch: string | undefined,
  currentEpoch: string,
): StartPosition | undefined {
  if (epoch !== undefined && since === undefined) throw new CliError('usage', '--epoch は --since と組で指定してください');
  if (since === undefined) return undefined;
  if (epoch !== undefined) return { since, epoch };
  writeLine(io.stderr, `[misao] 警告: --epoch が無いため、--since ${since} を現在の epoch (${currentEpoch}) の seq として扱います`);
  return { since, epoch: currentEpoch };
}

/**
 * ストリームを SIGINT まで追い続ける (終了コード 0)。切断は SDK の再接続と since 追従に任せ、
 * 再接続の経過と取りこぼし (gap) は stderr に出す。購読が拒否された・再接続を打ち切ったときは失敗にする。
 */
export async function follow(io: CliIo, client: MisaoClient, options: FollowOptions): Promise<number> {
  let finish: (outcome: number | CliError) => void = () => undefined;
  const finished = new Promise<number | CliError>((resolve) => {
    finish = resolve;
  });
  const warn = (text: string): void => writeLine(io.stderr, `[misao] ${text}`);
  let subscription: Subscription | undefined;
  // 購読の応答直後に届く再生分は subscription の代入より先に処理されうる。その間は購読時の epoch。
  const epochOf = (): string => (subscription === undefined ? options.initialEpoch : subscription.cursor.epoch);
  // 再接続したあとの since は seq が連続している前提なので、欠けたら必ず知らせる。
  let warnTruncated = options.warnTruncated;
  const cleanups = [
    io.onSignal('SIGINT', () => finish(0)),
    client.onStateChange((state) => {
      if (state.status === 'reconnecting') {
        warnTruncated = true;
        warn(`接続が切れました。再接続しています（${state.attempt} 回目）`);
      } else if (state.status === 'connected') {
        warn('再接続しました');
      } else {
        // closed: 再接続先が非互換 (cause あり) か、SDK が再接続を諦めた。どちらも追い続けられない。
        const reason = state.cause === undefined ? '' : `: ${state.cause.message}`;
        finish(new CliError('runtime', `再接続を打ち切りました${reason}`));
      }
    }),
    client.onGap((gap) => {
      if (gap.reason === 'epoch') {
        warn(`警告: epoch が指定したもの・前回と違うため（デーモンの再起動）、履歴を最初から読み直しました（epoch ${epochOf()}）`);
      } else if (warnTruncated) {
        warn(`警告: 取りこぼしがあります（epoch ${epochOf()}。指定した位置が保持範囲より古いため、残っている分から表示します）`);
      }
    }),
    client.onSubscriptionError((info) => finish(new CliError('rpc', `購読が終了しました: ${info.error.message}`))),
  ];
  try {
    subscription = await options.subscribe(client, epochOf);
    const outcome = await finished;
    if (outcome instanceof CliError) throw outcome;
    return outcome;
  } finally {
    subscription?.unsubscribe();
    for (const cleanup of cleanups) cleanup();
  }
}
