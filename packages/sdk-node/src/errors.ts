/** デーモンが JSON-RPC エラー応答を返した。 */
export class MisaoRpcError extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
    this.name = 'MisaoRpcError';
  }
}

/** 未接続・切断・プロトコル違反など、接続そのものの失敗。 */
export class MisaoConnectionError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = 'MisaoConnectionError';
  }
}

/** デーモンとクライアントのプロトコル major が合わない。 */
export class MisaoProtocolVersionError extends Error {
  constructor(
    readonly serverVersion: string,
    readonly clientVersion: string,
  ) {
    super(`incompatible protocol version: server ${serverVersion}, client ${clientVersion}`);
    this.name = 'MisaoProtocolVersionError';
  }
}
