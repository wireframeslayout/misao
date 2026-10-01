import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import { killChild, stopChild } from './child.js';
import { cleanEnv } from './daemon-process.js';
import { nodeCmd } from './node-cmd.js';
import { waitFor } from './wait.js';

const FAKE_HUB = new URL('../../src/fixtures/fake-hub.ts', import.meta.url).pathname;

export interface HubFiles {
  socket: string;
  stateFile: string;
  logFile: string;
}

export interface HubProcess {
  readonly pid: number;
  /** SIGTERM で止める。 */
  stop(): Promise<void>;
  /** SIGKILL で落とす。 */
  kill(): Promise<void>;
}

/** fake-hub を子プロセスで起動する。 */
export function startHubProcess({ socket, stateFile, logFile }: HubFiles): HubProcess {
  const [command, ...args] = nodeCmd(FAKE_HUB, '--socket', socket, '--state', stateFile, '--log', logFile);
  const child: ChildProcess = spawn(command!, args, { env: cleanEnv(), stdio: 'ignore' });
  if (child.pid === undefined) throw new Error('failed to spawn fake-hub');
  return { pid: child.pid, stop: () => stopChild(child), kill: () => killChild(child) };
}

export interface HubLogEntry {
  kind: string;
  seq?: number;
  type?: string;
  paneId?: string;
  text?: string;
  data?: Record<string, unknown>;
  [key: string]: unknown;
}

export function readHubLog(logFile: string): HubLogEntry[] {
  if (!fs.existsSync(logFile)) return [];
  return fs
    .readFileSync(logFile, 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line) as HubLogEntry);
}

/** events の購読が minCount 回 (再起動のたびに 1 回) ログに現れるまで待つ。 */
export async function waitForHubSubscribed(logFile: string, minCount: number): Promise<void> {
  await waitFor(
    () => readHubLog(logFile).filter((e) => e.kind === 'subscribed' && e.stream === 'events').length >= minCount,
    `the hub to subscribe (${minCount})`,
    { timeoutMs: 20_000 },
  );
}
