import * as fs from 'node:fs';
import * as z from 'zod';
import type { PaneRecord, WorkspaceDef } from './model.js';
import { DimensionSchema, LabelsSchema, PaneIdSchema, WindowIdSchema, WindowNameSchema, WorkspaceNameSchema } from '@misao/protocol';

const PersistedWindowSchema = z.strictObject({ id: WindowIdSchema, name: WindowNameSchema });

const PersistedWorkspaceSchema = z.strictObject({
  name: WorkspaceNameSchema,
  windows: z.array(PersistedWindowSchema),
});

const PersistedPaneSchema = z.strictObject({
  paneId: PaneIdSchema,
  windowId: WindowIdSchema,
  cmd: z.array(z.string()).min(1),
  cwd: z.string(),
  env: z.record(z.string(), z.string()),
  labels: LabelsSchema,
  cols: DimensionSchema,
  rows: DimensionSchema,
});

export interface PersistedState {
  version: 1;
  workspaces: WorkspaceDef[];
  panes: PaneRecord[];
}

/** workspace 名・window ID・pane ID の重複と、存在しない window への所属。問題が無ければ空。 */
export function checkConsistency(state: PersistedState): string[] {
  const issues: string[] = [];
  const windowIds = new Set(state.workspaces.flatMap((w) => w.windows.map((win) => win.id)));
  const windowCount = state.workspaces.reduce((n, w) => n + w.windows.length, 0);
  if (new Set(state.workspaces.map((w) => w.name)).size !== state.workspaces.length) issues.push('duplicate workspace name');
  if (windowIds.size !== windowCount) issues.push('duplicate window id');
  if (new Set(state.panes.map((p) => p.paneId)).size !== state.panes.length) issues.push('duplicate pane id');
  for (const p of state.panes) {
    if (!windowIds.has(p.windowId)) issues.push(`pane ${p.paneId}: unknown window ${p.windowId}`);
  }
  return issues;
}

const PersistedStateSchema = z
  .strictObject({
    version: z.literal(1),
    workspaces: z.array(PersistedWorkspaceSchema),
    panes: z.array(PersistedPaneSchema),
  })
  .superRefine((state, ctx) => {
    for (const message of checkConsistency(state)) ctx.addIssue({ code: 'custom', message });
  }) satisfies z.ZodType<PersistedState>;

/** ファイルが無ければ空の状態。壊れていたり未知の version なら例外 (起動を止める)。 */
export function loadPersistedState(path: string): PersistedState {
  let text: string;
  try {
    text = fs.readFileSync(path, 'utf8');
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === 'ENOENT') return { version: 1, workspaces: [], panes: [] };
    throw e;
  }
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch (e) {
    throw new Error(`invalid persistence file ${path}: ${(e as Error).message}`);
  }
  const result = PersistedStateSchema.safeParse(json);
  if (result.success) return result.data;
  const detail = result.error.issues.map((i) => `${i.path.length > 0 ? i.path.join('.') : 'state'}: ${i.message}`).join('; ');
  throw new Error(`invalid persistence file ${path}: ${detail}`);
}

/** 一時ファイルに書いて fsync してから rename する (途中で落ちても、電源断でも、元のファイルは壊れない)。 */
export function savePersistedState(path: string, state: PersistedState): void {
  const tmp = `${path}.tmp-${process.pid}`;
  try {
    const fd = fs.openSync(tmp, 'w', 0o600);
    try {
      fs.writeSync(fd, `${JSON.stringify(state, null, 2)}\n`);
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    fs.renameSync(tmp, path);
  } catch (e) {
    fs.rmSync(tmp, { force: true });
    throw e;
  }
}
