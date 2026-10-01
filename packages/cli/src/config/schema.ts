import * as z from 'zod';
import { parseKeySpec } from './keys.js';

const DEFAULT_SCROLLBACK = 5000;
const DEFAULT_RAW_RING_BYTES = 1024 * 1024;
const DEFAULT_LINES_RING_BYTES = 64 * 1024;
const DEFAULT_EVENT_RING_CAP = 1000;

const KeySpecSchema = z.string().transform((spec, ctx) => {
  try {
    return parseKeySpec(spec);
  } catch (error) {
    ctx.issues.push({
      code: 'custom',
      message: error instanceof Error ? error.message : String(error),
      input: spec,
    });
    return z.NEVER;
  }
});

const ACTION_KEYS = ['detach', 'next', 'prev', 'list'] as const;

const KeysSchema = z
  .strictObject({
    prefix: KeySpecSchema.prefault('C-^'),
    detach: KeySpecSchema.prefault('d'),
    next: KeySpecSchema.prefault('n'),
    prev: KeySpecSchema.prefault('p'),
    list: KeySpecSchema.prefault('l'),
  })
  .superRefine((keys, ctx) => {
    const seen = new Map<number, string>();
    for (const action of ACTION_KEYS) {
      const code = keys[action];
      const other = seen.get(code);
      if (other !== undefined) {
        ctx.addIssue({
          code: 'custom',
          message: `keys.${action} duplicates keys.${other}`,
          path: [action],
        });
      }
      seen.set(code, action);
      if (code === keys.prefix) {
        ctx.addIssue({
          code: 'custom',
          message: `keys.${action} collides with keys.prefix`,
          path: [action],
        });
      }
    }
  });

const RingsSchema = z.strictObject({
  rawBytes: z.int().min(1).default(DEFAULT_RAW_RING_BYTES),
  linesBytes: z.int().min(1).default(DEFAULT_LINES_RING_BYTES),
  events: z.int().min(1).default(DEFAULT_EVENT_RING_CAP),
});

export const MisaoConfigSchema = z.strictObject({
  keys: KeysSchema.prefault({}),
  scrollback: z.int().min(1).default(DEFAULT_SCROLLBACK),
  rings: RingsSchema.prefault({}),
  socket: z.string().min(1).optional(),
  logLevel: z.enum(['error', 'warn', 'info', 'debug']).default('info'),
});

export type MisaoConfig = z.output<typeof MisaoConfigSchema>;
export type KeyBindings = MisaoConfig['keys'];
