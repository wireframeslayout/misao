export const LOG_LEVELS = ['error', 'warn', 'info', 'debug'] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

export const DEFAULT_LOG_LEVEL: LogLevel = 'info';

export interface Logger {
  error(msg: string): void;
  warn(msg: string): void;
  info(msg: string): void;
  debug(msg: string): void;
}

/** level より詳細な出力を捨てて、残りを sink へ渡す。 */
export function createLogger(level: LogLevel, sink: (msg: string) => void): Logger {
  const threshold = LOG_LEVELS.indexOf(level);
  const at = (target: LogLevel) => (msg: string) => {
    if (LOG_LEVELS.indexOf(target) <= threshold) sink(msg);
  };
  return { error: at('error'), warn: at('warn'), info: at('info'), debug: at('debug') };
}
