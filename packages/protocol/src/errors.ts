import * as z from 'zod';

export const ErrorCode = {
  Parse: -32700,
  InvalidRequest: -32600,
  MethodNotFound: -32601,
  InvalidParams: -32602,
  Internal: -32603,
  PaneNotFound: 1001,
  PaneExited: 1002,
  Ambiguous: 1003,
  Unsupported: 1004,
  NotImplemented: 1005,
  WorkspaceNotFound: 1006,
  WindowNotFound: 1007,
  AlreadyExists: 1008,
} as const;

export type ErrorCodeValue = (typeof ErrorCode)[keyof typeof ErrorCode];

export const RpcErrorSchema = z.looseObject({
  code: z.int(),
  message: z.string(),
  data: z.unknown().optional(),
});
