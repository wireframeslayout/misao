import * as z from 'zod';
import { RpcErrorSchema } from './errors.js';

export const RpcIdSchema = z.union([z.number(), z.string()]);

export const RpcRequestSchema = z.looseObject({
  jsonrpc: z.literal('2.0'),
  id: RpcIdSchema,
  method: z.string(),
  params: z.record(z.string(), z.unknown()).optional(),
});

export const RpcSuccessResponseSchema = z.looseObject({
  jsonrpc: z.literal('2.0'),
  id: RpcIdSchema,
  result: z.unknown(),
  error: z.never().optional().describe('A response carries exactly one of result or error'),
});

export const RpcErrorResponseSchema = z.looseObject({
  jsonrpc: z.literal('2.0'),
  id: RpcIdSchema.nullable().describe('null when the request could not be parsed'),
  error: RpcErrorSchema,
  result: z.never().optional().describe('A response carries exactly one of result or error'),
});

export const RpcResponseSchema = z.union([RpcSuccessResponseSchema, RpcErrorResponseSchema]);

export const RpcNotificationSchema = z.looseObject({
  jsonrpc: z.literal('2.0'),
  id: z.never().optional().describe('A notification carries no id'),
  method: z.string(),
  params: z.record(z.string(), z.unknown()),
});

export const RpcMessageSchema = z.union([
  RpcRequestSchema,
  RpcResponseSchema,
  RpcNotificationSchema,
]);

export type RpcId = z.infer<typeof RpcIdSchema>;
export type RpcRequest = z.infer<typeof RpcRequestSchema>;
export type RpcSuccessResponse = z.infer<typeof RpcSuccessResponseSchema>;
export type RpcErrorResponse = z.infer<typeof RpcErrorResponseSchema>;
export type RpcResponse = z.infer<typeof RpcResponseSchema>;
export type RpcNotification = z.infer<typeof RpcNotificationSchema>;
export type RpcMessage = z.infer<typeof RpcMessageSchema>;

export function isResponse(m: RpcMessage): m is RpcResponse {
  return 'id' in m && !('method' in m);
}

export function isNotification(m: RpcMessage): m is RpcNotification {
  return 'method' in m && !('id' in m);
}

export function isRequest(m: RpcMessage): m is RpcRequest {
  return 'method' in m && 'id' in m;
}
