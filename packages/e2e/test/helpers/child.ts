import type { ChildProcess } from 'node:child_process';
import { sleep } from './wait.js';

const STOP_GRACE_MS = 8000;

function hasExited(child: ChildProcess): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

/** 起動した子を、記録した PID にだけ SIGTERM し、終わらなければ同じ PID に SIGKILL する。 */
export async function stopChild(child: ChildProcess): Promise<void> {
  if (hasExited(child)) return;
  const pid = child.pid;
  if (pid === undefined) throw new Error('child has no pid');
  const exited = new Promise<void>((resolve) => child.once('exit', () => resolve()));
  process.kill(pid, 'SIGTERM');
  await Promise.race([exited, sleep(STOP_GRACE_MS)]);
  if (hasExited(child)) return;
  process.kill(pid, 'SIGKILL');
  await exited;
}
