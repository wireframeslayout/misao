import { Listeners } from './listeners.js';

function logUnhandled(error: unknown): void {
  console.error('[misao-sdk] error thrown by a callback', error);
}

/**
 * 利用側コールバック (状態・gap・購読ハンドラなど) が投げた例外の報告先。
 *
 * 方針: 例外は SDK の内部処理を止めない。onError リスナーがあればそこへ渡し、無ければ
 * console.error で可視化する (握りつぶさない。再 throw もしない: 再接続ループやデータ経路を壊すため)。
 * onError リスナー自身が throw した場合は console.error に落とす (再帰しない)。
 */
export class ErrorChannel {
  private readonly listeners = new Listeners<unknown>(logUnhandled);

  add(callback: (error: unknown) => void): () => void {
    return this.listeners.add(callback);
  }

  /** 例外を報告する。自身は決して throw しない。 */
  readonly report = (error: unknown): void => {
    if (this.listeners.size === 0) logUnhandled(error);
    else this.listeners.emit(error);
  };
}
