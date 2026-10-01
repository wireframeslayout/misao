import type { MisaoClient } from '@misao/sdk';

/** デーモンの既定の workspace / window 名 (pane.open で windowId を省略したときの置き先)。 */
const DEFAULT_NAME = 'default';

/**
 * `new --workspace W --window NAME` の置き先 windowId を決める。どちらか片方だけなら、もう片方は既定名。
 * 無い workspace / window は作る。どちらも指定されなければ undefined (デーモンの既定に任せる)。
 */
export async function resolveWindowForOpen(
  client: MisaoClient,
  target: { workspace: string | undefined; window: string | undefined },
): Promise<string | undefined> {
  if (target.workspace === undefined && target.window === undefined) return undefined;
  const workspaceName = target.workspace ?? DEFAULT_NAME;
  const windowName = target.window ?? DEFAULT_NAME;
  const existing = (await client.request('workspace.list', {})).find((w) => w.name === workspaceName);
  const workspace = existing ?? (await client.request('workspace.create', { name: workspaceName }));
  const window =
    workspace.windows.find((w) => w.name === windowName) ??
    (await client.request('window.create', { workspace: workspaceName, name: windowName }));
  return window.windowId;
}
