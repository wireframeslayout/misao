import type { MisaoClient } from '@misao/sdk';

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** 条件が真になるまで待つ。時間内に満たなければ what を付けて throw する。 */
export async function waitFor(
  predicate: () => boolean | Promise<boolean>,
  what: string,
  { timeoutMs = 30_000, stepMs = 50 }: { timeoutMs?: number; stepMs?: number } = {},
): Promise<void> {
  const start = Date.now();
  while (!(await predicate())) {
    if (Date.now() - start > timeoutMs) throw new Error(`timeout waiting for ${what} (${timeoutMs}ms)`);
    await sleep(stepMs);
  }
}

/** pane.screen の activity (出力とリサイズで増える) が quietMs 動かなくなるまで待つ。 */
export async function waitQuiet(
  client: MisaoClient,
  paneId: string,
  { quietMs = 500, timeoutMs = 60_000 }: { quietMs?: number; timeoutMs?: number } = {},
): Promise<void> {
  let last = -1;
  let changedAt = Date.now();
  await waitFor(
    async () => {
      const { activity } = await client.request('pane.screen', { paneId });
      if (activity !== last) {
        last = activity;
        changedAt = Date.now();
      }
      return Date.now() - changedAt >= quietMs;
    },
    `pane ${paneId} to go quiet`,
    { timeoutMs, stepMs: 50 },
  );
}
