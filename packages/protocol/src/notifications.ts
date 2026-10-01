import * as z from 'zod';
import { PaneIdSchema, SeqSchema, TsSchema } from './primitives.js';

export const PaneOutputParamsSchema = z.looseObject({
  seq: SeqSchema,
  ts: TsSchema,
  paneId: PaneIdSchema,
  dataB64: z.string().describe('Raw PTY output, base64'),
  replay: z.literal('snapshot').optional().describe('Set when the data is a screen snapshot'),
});

export const PaneLineParamsSchema = z.looseObject({
  seq: SeqSchema,
  ts: TsSchema,
  paneId: PaneIdSchema,
  text: z.string(),
});

export const EventParamsSchema = z.looseObject({
  seq: SeqSchema,
  ts: TsSchema,
  type: z.string(),
  paneId: PaneIdSchema.optional(),
  data: z.looseObject({}),
});

export const notifications = {
  'pane.output': PaneOutputParamsSchema,
  'pane.line': PaneLineParamsSchema,
  event: EventParamsSchema,
} as const;

export type NotificationName = keyof typeof notifications;
export type NotificationParams<N extends NotificationName> = z.output<(typeof notifications)[N]>;
export type EventParams = z.output<typeof EventParamsSchema>;
