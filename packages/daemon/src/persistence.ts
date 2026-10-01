import * as fs from 'node:fs';
import * as z from 'zod';
import { DimensionSchema, LabelsSchema, PaneIdSchema, WindowIdSchema, WindowNameSchema, WorkspaceNameSchema } from '@misao/protocol';

const PersistedWindowSchema = z.strictObject({ id: WindowIdSchema, name: WindowNameSchema });

const PersistedWorkspaceSchema = z.strictObject({
  name: WorkspaceNameSchema,
  windows: z.array(PersistedWindowSchema),
});

/** env は pane.open の env パラメータ分のみ。ephemeralEnv は含めない。 */
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

/** workspace 名・window ID・pane ID の重複と、存在しない window への所属を、読み込み時に弾く。 */
const PersistedStateSchema = z
  .strictObject({
    version: z.literal(1),
    workspaces: z.array(PersistedWorkspaceSchema),
    panes: z.array(PersistedPaneSchema),
  })
  .superRefine((state, ctx) => {
    const windowIds = new Set(state.workspaces.flatMap((w) => w.windows.map((win) => win.id)));
    const workspaceNames = new Set(state.workspaces.map((w) => w.name));
    const windowCount = state.workspaces.reduce((n, w) => n + w.windows.length, 0);
    if (workspaceNames.size !== state.workspaces.length) ctx.addIssue({ code: 'custom', message: 'duplicate workspace name' });
    if (windowIds.size !== windowCount) ctx.addIssue({ code: 'custom', message: 'duplicate window id' });
    if (new Set(state.panes.map((p) => p.paneId)).size !== state.panes.length) {
      ctx.addIssue({ code: 'custom', message: 'duplicate pane id' });
    }
    for (const p of state.panes) {
      if (!windowIds.has(p.windowId)) ctx.addIssue({ code: 'custom', message: `pane ${p.paneId}: unknown window ${p.windowId}` });
    }
  });

export type PersistedWorkspace = z.output<typeof PersistedWorkspaceSchema>;
export type PersistedPane = z.output<typeof PersistedPaneSchema>;
export type PersistedState = z.output<typeof PersistedStateSchema>;

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

/** 一時ファイルに書いてから rename する (途中で落ちても元のファイルは壊れない)。 */
export function savePersistedState(path: string, state: PersistedState): void {
  const tmp = `${path}.tmp-${process.pid}`;
  try {
    fs.writeFileSync(tmp, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 });
    fs.renameSync(tmp, path);
  } catch (e) {
    fs.rmSync(tmp, { force: true });
    throw e;
  }
}
