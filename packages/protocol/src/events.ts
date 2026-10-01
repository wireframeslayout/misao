import * as z from 'zod';
import type { EventParams } from './notifications.js';
import { AgentStateSchema } from './pane-info.js';
import {
  ClientIdSchema,
  DimensionSchema,
  ReportedInputSourceSchema,
  LabelsSchema,
  WindowIdSchema,
  WindowNameSchema,
  WorkspaceNameSchema,
} from './primitives.js';

const WorkspaceRenamedSchema = z.looseObject({
  name: WorkspaceNameSchema,
  newName: WorkspaceNameSchema,
});
const WorkspaceNamedSchema = z.looseObject({ name: WorkspaceNameSchema });
const ClientEventSchema = z.looseObject({ clientId: ClientIdSchema });

/** type から data スキーマを引く表。pane に紐づくイベントの paneId は外側に載る。 */
export const knownEventData = {
  'daemon.started': z.looseObject({ pid: z.int(), protocolVersion: z.string() }),
  'pane.opened': z.looseObject({
    pid: z.int(),
    cmd: z.array(z.string()),
    labels: LabelsSchema,
    workspace: WorkspaceNameSchema,
    windowId: WindowIdSchema,
  }),
  'pane.title': z.looseObject({ title: z.string() }),
  'pane.exited': z.looseObject({ exitCode: z.int().nullable(), signal: z.int().nullable() }),
  'pane.resized': z.looseObject({
    cols: DimensionSchema,
    rows: DimensionSchema,
    clientId: ClientIdSchema.nullable(),
  }),
  'pane.closed': z.looseObject({}),
  'pane.state': z.looseObject({
    state: AgentStateSchema,
    decidedBy: z.string(),
    prev: AgentStateSchema.optional(),
  }),
  'pane.label': z.looseObject({ set: LabelsSchema, unset: z.array(z.string()) }),
  input: z
    .looseObject({ source: ReportedInputSourceSchema, bytes: z.int().min(0) })
    .describe('The input content itself is not carried'),
  'client.attached': ClientEventSchema,
  'client.detached': ClientEventSchema,
  'workspace.created': WorkspaceNamedSchema,
  'workspace.closed': WorkspaceNamedSchema,
  'workspace.renamed': WorkspaceRenamedSchema,
  'window.created': z.looseObject({
    windowId: WindowIdSchema,
    workspace: WorkspaceNameSchema,
    name: WindowNameSchema,
  }),
  'window.closed': z.looseObject({ windowId: WindowIdSchema }),
  'window.renamed': z.looseObject({ windowId: WindowIdSchema, name: WindowNameSchema }),
  focus: z.looseObject({ windowId: WindowIdSchema, clientId: ClientIdSchema }),
} as const;

export type KnownEventType = keyof typeof knownEventData;
export type KnownEventData<T extends KnownEventType> = z.output<(typeof knownEventData)[T]>;

export type KnownEvent = {
  [T in KnownEventType]: Pick<EventParams, 'seq' | 'ts' | 'paneId'> & { type: T; data: KnownEventData<T> };
}[KnownEventType];

function isKnownEventType(type: string): type is KnownEventType {
  return Object.hasOwn(knownEventData, type);
}

/**
 * 既知の type なら data を検証して型付きで返す (合わなければ ZodError)。
 * 未知の type は undefined。
 */
export function parseKnownEvent(ev: EventParams): KnownEvent | undefined {
  if (!isKnownEventType(ev.type)) return undefined;
  const data = knownEventData[ev.type].parse(ev.data);
  return { ...ev, type: ev.type, data } as KnownEvent;
}
