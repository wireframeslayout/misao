import type { ErrorCodeValue } from '@misao/protocol';

/** ハンドラが投げる、JSON-RPC のエラー応答になる例外。 */
export class RpcFailure extends Error {
  constructor(
    readonly code: ErrorCodeValue,
    message: string,
  ) {
    super(message);
  }
}
