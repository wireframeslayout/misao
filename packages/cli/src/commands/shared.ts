import type { MisaoClient } from '@misao/sdk';
import type { PaneInfo } from '@misao/protocol';
import type { ParsedArgs } from '../args.js';
import { CliError } from '../errors.js';
import { resolveTarget } from '../target.js';

/** 位置引数の個数を検証する。足りない・多いときは使い方を添えた usage エラー。 */
export function expectPositionals(args: ParsedArgs, min: number, max: number, usage: string): string[] {
  if (args.positionals.length < min || args.positionals.length > max) {
    throw new CliError('usage', `使い方: ${usage}`);
  }
  return [...args.positionals];
}

/** 全ペインを取得して、対象指定から 1 つに決める。 */
export async function resolvePane(client: MisaoClient, query: string, homeDir: string): Promise<PaneInfo> {
  const panes = await client.request('pane.list', {});
  return resolveTarget(query, panes, homeDir);
}
