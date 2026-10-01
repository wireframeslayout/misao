/** Daemon の上限値の既定。CLI の設定スキーマもここを参照する。 */
export const DEFAULT_DAEMON_LIMITS = {
  scrollback: 5000,
  rawRingBytes: 1024 * 1024,
  // 行ストリームは遅い購読者の遅れをこのリングが受け持つ (追い越されたときだけ切断 → gap)。
  // 容量は 1 行ごとに文字数 + LINE_ENTRY_OVERHEAD (64) で数える。購読者のいないペインに流してリングが満杯になるまで、
  // gc 後の heapUsed の増分を実測した値 (1 ペインあたり、node 24):
  //   平均 60 文字の行: 約 5 MiB (約 3.4 万行)、空行のみ: 約 5 MiB (約 6.5 万行)
  // 修正前 (size = 文字数 + 1、1 文字ずつ連結) は 107 MiB / 296 MiB だった。ペイン数をかけた値が上限の目安。
  linesRingBytes: 4 * 1024 * 1024,
  events: 1000,
} as const;

export interface ResolvedDaemonLimits {
  scrollback: number;
  rawRingBytes: number;
  linesRingBytes: number;
  events: number;
}

export interface DaemonLimitOptions {
  scrollback?: number;
  rings?: { rawBytes?: number; linesBytes?: number; events?: number };
}

/** 既定値で補い、正の整数でなければ RangeError。 */
export function resolveDaemonLimits(opts: DaemonLimitOptions): ResolvedDaemonLimits {
  const d = DEFAULT_DAEMON_LIMITS;
  return {
    scrollback: positiveInt('scrollback', opts.scrollback ?? d.scrollback),
    rawRingBytes: positiveInt('rings.rawBytes', opts.rings?.rawBytes ?? d.rawRingBytes),
    linesRingBytes: positiveInt('rings.linesBytes', opts.rings?.linesBytes ?? d.linesRingBytes),
    events: positiveInt('rings.events', opts.rings?.events ?? d.events),
  };
}

function positiveInt(name: string, value: number): number {
  if (!Number.isInteger(value) || value < 1) throw new RangeError(`${name} must be a positive integer: ${value}`);
  return value;
}
