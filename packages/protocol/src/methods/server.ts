import * as z from 'zod';
import { EpochSchema, SeqSchema } from '../primitives.js';

export const serverMethods = {
  'server.info': {
    params: z.object({}),
    result: z.looseObject({
      protocolVersion: z.string(),
      // デーモンのビルド版。version を返さない古いデーモンもあるので省略可。
      version: z.string().optional(),
      pid: z.int(),
      epoch: EpochSchema,
      uptimeSec: z.number().min(0),
      paneCount: z.int().min(0),
      eventHead: SeqSchema,
    }),
  },
  'server.schema': {
    params: z.object({}),
    result: z
      .looseObject({ protocolVersion: z.string() })
      .describe('The JSON schema of the whole protocol'),
  },
};
