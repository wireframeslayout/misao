/** 子プロセス環境: 親の環境から mux 系の変数を除去し、misao 用を足す。overrides が最後に上書きする。 */
export function buildChildEnv(
  base: NodeJS.ProcessEnv,
  paneId: string,
  socketPath: string,
  overrides: Record<string, string> = {},
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(base)) {
    if (v === undefined) continue;
    if (k === 'TMUX' || k === 'TMUX_PANE' || k === 'STY' || k.startsWith('ZELLIJ')) continue;
    env[k] = v;
  }
  env.MISAO_SOCKET = socketPath;
  env.MISAO_PANE_ID = paneId;
  env.TERM = 'xterm-256color';
  env.COLORTERM = 'truecolor';
  return { ...env, ...overrides };
}
