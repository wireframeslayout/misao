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

const PROCESS_STATES = ['running', 'exited', 'stopped'] as const;

/** 送信側 (params) 用。未知の値は拒否する。 */
export const ProcessStateSchema = z
  .enum(PROCESS_STATES)
  .describe(
    'Process state. "stopped": the daemon restarted, the PTY was lost and only metadata remains.',
  );

/** 受信側 (result / 通知) 用。新しいデーモンが追加した未知の値は "unknown" として読む。 */
export const ReportedProcessStateSchema = z
  .enum([...PROCESS_STATES, 'unknown'])
  .catch('unknown')
  .describe(
    'Process state. "stopped": the daemon restarted, the PTY was lost and only metadata remains. The daemon never sends "unknown"; receivers read values they do not know as "unknown".',
  );

/** 受信側でのみ使う。未知の値は "unknown" として読む。 */
export const AgentStateSchema = z
  .enum(['working', 'blocked', 'idle', 'exited', 'unknown'])
  .catch('unknown')
  .describe('Agent state. Receivers read values they do not know as "unknown".');

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
  processState: ReportedProcessStateSchema,
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
export type ReportedProcessState = z.infer<typeof ReportedProcessStateSchema>;
export type AgentState = z.infer<typeof AgentStateSchema>;
export type PaneInfo = z.infer<typeof PaneInfoSchema>;
