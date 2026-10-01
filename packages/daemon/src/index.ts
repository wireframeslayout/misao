export const PACKAGE = '@misao/daemon';
export { Daemon } from './daemon.js';
export type { DaemonOptions } from './daemon.js';
export { ensureSocketDir, listenUnixSocket } from './socket.js';
