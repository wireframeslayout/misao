import type * as z from 'zod';
import { ErrorCode, methods } from '@misao/protocol';
import type { MethodName } from '@misao/protocol';
import { RpcFailure } from './rpc-error.js';

export type ParsedParams<M extends MethodName> = z.output<(typeof methods)[M]['params']>;

/** protocol の zod スキーマで params を検証する。違反は -32602（issue は `path: message` にまとめる）。 */
export function parseParams<M extends MethodName>(method: M, raw: Record<string, unknown> | undefined): ParsedParams<M> {
  const result = methods[method].params.safeParse(raw ?? {});
  if (result.success) return result.data as ParsedParams<M>;
  const detail = result.error.issues
    .map((issue) => `${issue.path.length > 0 ? issue.path.join('.') : 'params'}: ${issue.message}`)
    .join('; ');
  throw new RpcFailure(ErrorCode.InvalidParams, `invalid params for ${method}: ${detail}`);
}
