import { MisaoRpcError } from '@misao/sdk';
import { ConfigError } from './config/index.js';

export type CliErrorKind =
  | 'usage'
  | 'not_found'
  | 'ambiguous'
  | 'aborted'
  | 'daemon_unreachable'
  | 'rpc'
  | 'config'
  | 'runtime';

export interface ErrorCandidate {
  paneId: string;
  name: string;
}

/** コマンドが利用者へ伝える失敗。終了コードは usage だけ 2、それ以外は 1。 */
export class CliError extends Error {
  constructor(
    readonly kind: CliErrorKind,
    message: string,
    readonly candidates?: readonly ErrorCandidate[],
  ) {
    super(message);
    this.name = 'CliError';
  }

  get exitCode(): number {
    return this.kind === 'usage' ? 2 : 1;
  }
}

/** run の境界で、コマンドから伝播した例外を CliError にそろえる。 */
export function toCliError(error: unknown): CliError {
  if (error instanceof CliError) return error;
  if (error instanceof ConfigError) return new CliError('config', error.message);
  if (error instanceof MisaoRpcError) return new CliError('rpc', error.message);
  return new CliError('runtime', error instanceof Error ? error.message : String(error));
}
