import * as z from 'zod';
import {
  ClientIdSchema,
  DimensionSchema,
  LabelsSchema,
  OkResultSchema,
  PaneIdSchema,
  SeqSchema,
  SinceSchema,
  SubscribeResultSchema,
  WindowIdSchema,
  WorkspaceNameSchema,
} from '../primitives.js';
import { PaneInfoSchema, ProcessStateSchema } from '../pane-info.js';

const InputSourceSchema = z.enum(['hub', 'terminal']);

export const PreplaceFileSchema = z.object({
  path: z
    .string()
    .regex(/^(?![/\\])(?!(?:.*[/\\])?\.\.(?:[/\\]|$)).+$/)
    .describe(
      'Path relative to the temporary directory. Absolute paths and ".." segments are rejected.',
    ),
  content: z.string().describe('File content (UTF-8 string)'),
  mode: z.int().min(0).max(0o777).optional().describe('File mode (0 to 0o777)'),
});

export const PaneListFilterSchema = z
  .object({
    state: ProcessStateSchema.optional(),
    labels: LabelsSchema.optional().describe('Matches when every key/value is equal'),
    workspace: WorkspaceNameSchema.optional(),
  })
  .describe('All conditions are ANDed');

const PaneRefSchema = z.object({ paneId: PaneIdSchema });

const writeBase = {
  paneId: PaneIdSchema,
  clientId: ClientIdSchema.optional(),
  source: InputSourceSchema.optional(),
};

export const paneMethods = {
  'pane.open': {
    params: z.object({
      cmd: z.array(z.string()).min(1),
      cwd: z.string().optional(),
      env: z.record(z.string(), z.string()).optional(),
      cols: DimensionSchema.optional(),
      rows: DimensionSchema.optional(),
      labels: LabelsSchema.optional(),
      window: WindowIdSchema.optional().describe(
        'Target window. Defaults to the default workspace/window.',
      ),
      preplace: z
        .array(PreplaceFileSchema)
        .optional()
        .describe(
          'Files placed in a temporary directory created outside cwd. The caller decides the content.',
        ),
    }),
    result: z.looseObject({ paneId: PaneIdSchema }),
  },
  'pane.info': { params: PaneRefSchema, result: PaneInfoSchema },
  'pane.list': {
    params: z.object({ filter: PaneListFilterSchema.optional() }),
    result: z.array(PaneInfoSchema),
  },
  'pane.write': {
    params: z.union([
      z.object({ ...writeBase, data: z.string(), dataB64: z.never().optional() }),
      z.object({ ...writeBase, dataB64: z.string(), data: z.never().optional() }),
    ]),
    result: OkResultSchema,
  },
  'pane.send_keys': {
    params: z
      .object({
        paneId: PaneIdSchema,
        keys: z
          .array(z.string())
          .min(1)
          .describe('Key names, e.g. Enter, Escape, Tab, C-c, Up, Down'),
        clientId: ClientIdSchema.optional(),
        source: InputSourceSchema.optional(),
      }),
    result: OkResultSchema,
  },
  'pane.resize': {
    params: z.object({
      paneId: PaneIdSchema,
      cols: DimensionSchema,
      rows: DimensionSchema,
      clientId: ClientIdSchema.optional(),
    }),
    result: OkResultSchema,
  },
  'pane.screen': {
    params: PaneRefSchema,
    result: z.looseObject({
      text: z.string(),
      cursor: z.looseObject({ x: z.int().min(0), y: z.int().min(0) }),
      altScreen: z.boolean(),
      title: z.string(),
    }),
  },
  'pane.set_label': {
    params: z.union([
      z.object({
        paneId: PaneIdSchema,
        set: LabelsSchema,
        unset: z.array(z.string()).optional(),
      }),
      z.object({
        paneId: PaneIdSchema,
        set: LabelsSchema.optional(),
        unset: z.array(z.string()),
      }),
    ]),
    result: z.looseObject({ labels: LabelsSchema.describe('Labels after applying the change') }),
  },
  'pane.attach': {
    params: z.object({
      paneId: PaneIdSchema,
      clientId: ClientIdSchema,
      mode: z
        .enum(['raw', 'cells'])
        .default('raw')
        .describe('"cells" is reserved only: the daemon returns 1004 Unsupported.'),
      replay: z.enum(['raw', 'snapshot', 'none']).default('none'),
      cols: DimensionSchema.optional(),
      rows: DimensionSchema.optional(),
    }),
    result: z.looseObject({
      headSeq: SeqSchema,
      oldest: SeqSchema,
      truncated: z.boolean(),
    }),
  },
  'pane.detach': { params: PaneRefSchema, result: OkResultSchema },
  'pane.subscribe_lines': {
    params: z.object({ paneId: PaneIdSchema, since: SinceSchema.optional() }),
    result: SubscribeResultSchema,
  },
  'pane.close': { params: PaneRefSchema, result: OkResultSchema },
  'pane.respawn': {
    params: z
      .object({ paneId: PaneIdSchema, cmd: z.array(z.string()).min(1).optional() })
      .describe('Schema only. The Phase 1 daemon returns 1005 NotImplemented.'),
    result: z.looseObject({ paneId: PaneIdSchema }),
  },
  'events.subscribe': {
    params: z.object({ since: SinceSchema.optional() }),
    result: SubscribeResultSchema,
  },
};
