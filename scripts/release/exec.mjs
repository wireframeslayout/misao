import { spawnSync } from 'node:child_process';

// コマンドを実行し、失敗（起動失敗・非ゼロ終了）は例外にして呼び出し元へ伝える。
export function run(command, args, { cwd, capture = false, env } = {}) {
  const result = spawnSync(command, args, {
    cwd,
    env: env === undefined ? process.env : { ...process.env, ...env },
    encoding: 'utf8',
    stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} exited with ${result.status}`);
  }
  return result.stdout;
}
