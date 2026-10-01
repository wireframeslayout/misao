import { DEFAULT_DAEMON_LIMITS, DEFAULT_LOG_LEVEL, LOG_LEVELS } from '@misao/daemon';
import * as z from 'zod';
import { parseKeySpec, parsePrefixKeySpec } from './keys.js';

function keySpecSchema(parse: (spec: string) => number) {
  return z.string().transform((spec, ctx) => {
    try {
      return parse(spec);
    } catch (error) {
      ctx.issues.push({
        code: 'custom',
        message: error instanceof Error ? error.message : String(error),
        input: spec,
      });
      return z.NEVER;
    }
  });
}

const KeySpecSchema = keySpecSchema(parseKeySpec);
const PrefixKeySpecSchema = keySpecSchema(parsePrefixKeySpec);

const ACTION_KEYS = ['detach', 'next', 'prev', 'list'] as const;

const KeysSchema = z
  .strictObject({
    prefix: PrefixKeySpecSchema.prefault('C-^'),
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
  rawBytes: z.int().min(1).default(DEFAULT_DAEMON_LIMITS.rawRingBytes),
  linesBytes: z.int().min(1).default(DEFAULT_DAEMON_LIMITS.linesRingBytes),
  events: z.int().min(1).default(DEFAULT_DAEMON_LIMITS.events),
});

export const MisaoConfigSchema = z.strictObject({
  keys: KeysSchema.prefault({}),
  scrollback: z.int().min(1).default(DEFAULT_DAEMON_LIMITS.scrollback),
  rings: RingsSchema.prefault({}),
  socket: z.string().min(1).optional(),
  logLevel: z.enum(LOG_LEVELS).default(DEFAULT_LOG_LEVEL),
});

export type MisaoConfig = z.output<typeof MisaoConfigSchema>;
export type KeyBindings = MisaoConfig['keys'];
