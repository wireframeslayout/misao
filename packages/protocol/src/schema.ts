import * as z from 'zod';
import { ErrorCode } from './errors.js';
import { knownEventData } from './events.js';
import { RpcMessageSchema } from './jsonrpc.js';
import { methods } from './methods/index.js';
import { notifications } from './notifications.js';
import { PROTOCOL_VERSION } from './version.js';

export type JsonSchema = z.core.JSONSchema.BaseSchema;

export interface ProtocolJsonSchema {
  protocolVersion: string;
  jsonrpc: JsonSchema;
  methods: Record<string, { params: JsonSchema; result: JsonSchema }>;
  notifications: Record<string, JsonSchema>;
  events: Record<string, JsonSchema>;
  errors: Record<string, number>;
}

function mapValues<T, U>(obj: Record<string, T>, fn: (v: T) => U): Record<string, U> {
  return Object.fromEntries(Object.entries(obj).map(([k, v]) => [k, fn(v)]));
}

/** params は io: 'input' (既定値を省略可能にする)、それ以外は io: 'output'。 */
export function buildProtocolJsonSchema(): ProtocolJsonSchema {
  return {
    protocolVersion: PROTOCOL_VERSION,
    jsonrpc: z.toJSONSchema(RpcMessageSchema, { io: 'output' }),
    methods: mapValues(methods, (m: { params: z.ZodType; result: z.ZodType }) => ({
      params: z.toJSONSchema(m.params, { io: 'input' }),
      result: z.toJSONSchema(m.result, { io: 'output' }),
    })),
    notifications: mapValues(notifications, (s: z.ZodType) => z.toJSONSchema(s, { io: 'output' })),
    events: mapValues(knownEventData, (s: z.ZodType) => z.toJSONSchema(s, { io: 'output' })),
    errors: { ...ErrorCode },
  };
}
