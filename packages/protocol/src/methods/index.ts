import type * as z from 'zod';
import { paneMethods } from './pane.js';
import { serverMethods } from './server.js';
import { workspaceMethods } from './workspace.js';

export interface MethodDef {
  params: z.ZodType;
  result: z.ZodType;
}

export const methods = {
  ...serverMethods,
  ...workspaceMethods,
  ...paneMethods,
} as const satisfies Record<string, MethodDef>;

export type MethodName = keyof typeof methods;
export type MethodParams<M extends MethodName> = z.input<(typeof methods)[M]['params']>;
export type MethodResult<M extends MethodName> = z.output<(typeof methods)[M]['result']>;
