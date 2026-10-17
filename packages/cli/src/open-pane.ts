import type { MisaoClient } from '@misao/sdk';
import { CliError } from './errors.js';

export interface OpenPaneRequest {
  cmd: string[];
  cwd: string;
  labels: Record<string, string>;
  env: Record<string, string>;
  /** 置き先の窓。省略時はデーモンの既定の窓。 */
  windowId: string | undefined;
}

/** ログインシェルの起動コマンド。シェルが取れない環境では usage エラー。Windows の cmd.exe は -l を受け付けないので付けない。 */
export function loginShell(shell: string | null, platform: NodeJS.Platform = process.platform): string[] {
  if (shell === null) throw new CliError('usage', 'ログインシェルを特定できません。-- の後ろにコマンドを指定してください');
  return platform === 'win32' ? [shell] : [shell, '-l'];
}

/** 端末から作るペインを開く。origin=terminal ラベルを必ず付ける (未登録のペインを見分けるため)。 */
export async function openTerminalPane(client: MisaoClient, request: OpenPaneRequest): Promise<string> {
  const { paneId } = await client.request('pane.open', {
    cmd: request.cmd,
    cwd: request.cwd,
    labels: { ...request.labels, origin: 'terminal' },
    ...(Object.keys(request.env).length > 0 ? { env: request.env } : {}),
    ...(request.windowId === undefined ? {} : { windowId: request.windowId }),
  });
  return paneId;
}
