export interface BackoffOptions {
  initialDelayMs: number;
  maxDelayMs: number;
  factor: number;
}

export const DEFAULT_BACKOFF: BackoffOptions = { initialDelayMs: 100, maxDelayMs: 5000, factor: 2 };

/** attempt は 1 始まりの再接続試行回数。initialDelayMs * factor^(attempt-1) を maxDelayMs で頭打ちにする。 */
export function computeBackoffDelay(attempt: number, options: BackoffOptions): number {
  if (!Number.isInteger(attempt) || attempt < 1) throw new RangeError(`attempt must be an integer >= 1: ${attempt}`);
  return Math.min(options.maxDelayMs, options.initialDelayMs * options.factor ** (attempt - 1));
}
