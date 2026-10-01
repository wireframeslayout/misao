import type { MisaoClient, Subscription } from '@misao/sdk';
import type { CliIo } from './cli-io.js';
import { CliError } from './errors.js';
import { writeLine } from './output.js';

export interface FollowOptions {
  /** 最初の購読で、保持範囲外 (truncated) の gap を警告するか。--since を指定したときだけ true。 */
  warnTruncated: boolean;
  subscribe(client: MisaoClient): Promise<Subscription>;
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
      } else if (state.cause !== undefined) {
        finish(new CliError('runtime', `再接続を打ち切りました: ${state.cause.message}`));
      }
    }),
    client.onGap((gap) => {
      if (gap.reason === 'epoch') warn('警告: デーモンが再起動したため、履歴を最初から読み直しました');
      else if (warnTruncated) warn('警告: 取りこぼしがあります（指定した位置が保持範囲より古いため、残っている分から表示します）');
    }),
    client.onSubscriptionError((info) => finish(new CliError('rpc', `購読が終了しました: ${info.error.message}`))),
  ];
  let subscription: Subscription | undefined;
  try {
    subscription = await options.subscribe(client);
    const outcome = await finished;
    if (outcome instanceof CliError) throw outcome;
    return outcome;
  } finally {
    subscription?.unsubscribe();
    for (const cleanup of cleanups) cleanup();
  }
}
