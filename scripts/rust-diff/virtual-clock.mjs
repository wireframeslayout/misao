// 仮想時間のタイマー。setTimeout / setInterval / Date.now を差し替え、tick(ms) で時刻を進めながら
// 期限の来たタイマーを (期限, 登録の順) に実行する。node:test のモックタイマーは、setInterval がある間
// xterm の setTimeout(0) を実行しないことがあったので使わない。

export function installVirtualClock(start) {
  const real = {
    setTimeout: globalThis.setTimeout,
    setInterval: globalThis.setInterval,
    clearTimeout: globalThis.clearTimeout,
    clearInterval: globalThis.clearInterval,
    now: Date.now,
  };
  let now = start;
  let seq = 0;
  const timers = new Map();

  const add = (fn, delay, repeat) => {
    const id = ++seq;
    const timer = { id, at: now + Math.max(0, Number(delay) || 0), seq: id, fn, every: repeat ? Math.max(1, Number(delay) || 1) : 0 };
    timers.set(id, timer);
    return { unref() { return this; }, ref() { return this; }, hasRef: () => true, [Symbol.toPrimitive]: () => id, _id: id };
  };
  const clear = (handle) => {
    if (handle && handle._id !== undefined) timers.delete(handle._id);
  };

  globalThis.setTimeout = (fn, delay, ...args) => add(() => fn(...args), delay, false);
  globalThis.setInterval = (fn, delay, ...args) => add(() => fn(...args), delay, true);
  globalThis.clearTimeout = clear;
  globalThis.clearInterval = clear;
  Date.now = () => now;

  return {
    now: () => now,
    tick(ms) {
      const target = now + ms;
      for (;;) {
        let next;
        for (const t of timers.values()) {
          if (t.at <= target && (!next || t.at < next.at || (t.at === next.at && t.seq < next.seq))) next = t;
        }
        if (!next) break;
        now = Math.max(now, next.at);
        if (next.every > 0) {
          next.at += next.every;
          next.seq = ++seq;
        } else {
          timers.delete(next.id);
        }
        next.fn();
      }
      now = target;
    },
    uninstall() {
      Object.assign(globalThis, {
        setTimeout: real.setTimeout,
        setInterval: real.setInterval,
        clearTimeout: real.clearTimeout,
        clearInterval: real.clearInterval,
      });
      Date.now = real.now;
    },
  };
}
