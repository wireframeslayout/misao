import * as z from 'zod';
import {
  ClientIdSchema,
  OkResultSchema,
  WindowIdSchema,
  WindowNameSchema,
  WorkspaceNameSchema,
} from '../primitives.js';

export const WindowInfoSchema = z.looseObject({
  windowId: WindowIdSchema,
  name: WindowNameSchema,
  workspace: WorkspaceNameSchema,
});

export const WorkspaceInfoSchema = z.looseObject({
  name: WorkspaceNameSchema,
  windows: z.array(WindowInfoSchema),
});

export type WindowInfo = z.infer<typeof WindowInfoSchema>;
export type WorkspaceInfo = z.infer<typeof WorkspaceInfoSchema>;

export const workspaceMethods = {
  'workspace.list': { params: z.object({}), result: z.array(WorkspaceInfoSchema) },
  'workspace.create': {
    params: z.object({ name: WorkspaceNameSchema }),
    result: WorkspaceInfoSchema,
  },
  'workspace.close': { params: z.object({ name: WorkspaceNameSchema }), result: OkResultSchema },
  'workspace.rename': {
    params: z.object({ name: WorkspaceNameSchema, newName: WorkspaceNameSchema }),
    result: OkResultSchema,
  },
  'window.create': {
    params: z.object({ workspace: WorkspaceNameSchema, name: WindowNameSchema }),
    result: WindowInfoSchema,
  },
  'window.close': { params: z.object({ windowId: WindowIdSchema }), result: OkResultSchema },
  'window.rename': {
    params: z.object({ windowId: WindowIdSchema, name: WindowNameSchema }),
    result: OkResultSchema,
  },
  'window.focus': {
    params: z
      .object({ windowId: WindowIdSchema, clientId: ClientIdSchema })
      .describe('Focus is tracked per client'),
    result: OkResultSchema,
  },
};
