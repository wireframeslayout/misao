import * as z from 'zod';
import {
  ClientIdSchema,
  DimensionSchema,
  LabelsSchema,
  PaneIdSchema,
  TsSchema,
  WindowIdSchema,
  WindowNameSchema,
  WorkspaceNameSchema,
} from './primitives.js';

export const ProcessStateSchema = z
  .enum(['running', 'exited', 'stopped'])
  .describe(
    'Process state. "stopped": the daemon restarted, the PTY was lost and only metadata remains.',
  );

export const AgentStateSchema = z.enum(['working', 'blocked', 'idle', 'exited', 'unknown']);

export const WindowRefSchema = z.looseObject({
  id: WindowIdSchema,
  name: WindowNameSchema,
});

export const PaneInfoSchema = z.looseObject({
  paneId: PaneIdSchema,
  pid: z.int().nullable().describe('null when processState is "stopped"'),
  cmd: z.array(z.string()),
  cwd: z.string(),
  workspace: WorkspaceNameSchema,
  window: WindowRefSchema,
  labels: LabelsSchema,
  processState: ProcessStateSchema,
  exitCode: z.int().nullable(),
  signal: z.int().nullable(),
  agentState: AgentStateSchema,
  decidedBy: z.string(),
  fgCommand: z.string().optional(),
  title: z.string(),
  lastOutputAt: TsSchema.nullable(),
  cols: DimensionSchema,
  rows: DimensionSchema,
  clients: z.array(ClientIdSchema),
  sizeOwner: ClientIdSchema.nullable(),
});

export type ProcessState = z.infer<typeof ProcessStateSchema>;
export type AgentState = z.infer<typeof AgentStateSchema>;
export type PaneInfo = z.infer<typeof PaneInfoSchema>;
