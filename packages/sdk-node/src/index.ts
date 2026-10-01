export const PACKAGE = '@misao/sdk';
export { MAX_SOCKET_PATH_BYTES, MisaoPathError, resolveMisaoDirs, resolveSocketPath } from './socket-path.js';
export type {
  MisaoDir,
  MisaoDirOrigin,
  ResolveMisaoDirsInput,
  ResolveSocketPathInput,
  SocketPathEnv,
} from './socket-path.js';
export { MisaoClient } from './client.js';
export type { ConnectionState, MisaoClientOptions } from './client.js';
export { DEFAULT_BACKOFF, computeBackoffDelay } from './backoff.js';
export type { BackoffOptions } from './backoff.js';
export { MisaoConnectionError, MisaoProtocolVersionError, MisaoRpcError } from './errors.js';
export type { Notification } from './rpc-connection.js';
export type {
  EventHandler,
  LineHandler,
  StreamId,
  StreamPosition,
} from './stream-cursor.js';
export type {
  GapInfo,
  GapReason,
  SubscribeOptions,
  Subscription,
  SubscriptionErrorInfo,
} from './stream-subscriber.js';
