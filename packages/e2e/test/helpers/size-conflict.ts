import assert from 'node:assert/strict';
import type { MisaoClient } from '@misao/sdk';
import { paneInfo } from './agents.js';
import { ClientView } from './client-view.js';
import { waitFor, waitQuiet } from './wait.js';

const SIZE_A = { cols: 120, rows: 40 };
const SIZE_B = { cols: 80, rows: 24 };

interface Expectation {
  step: string;
  size: { cols: number; rows: number };
  owner: string;
  /** 画面がデーモンと一致すべき視点 (pane サイズの所有者)。 */
  view: ClientView;
}

async function assertStep(client: MisaoClient, paneId: string, { step, size, owner, view }: Expectation): Promise<void> {
  await waitFor(
    async () => {
      const info = await paneInfo(client, paneId);
      return info.cols === size.cols && info.rows === size.rows && info.sizeOwner === owner;
    },
    `${step}: pane ${size.cols}x${size.rows} owned by ${owner}`,
    { timeoutMs: 10_000 },
  );
  await waitQuiet(client, paneId, { quietMs: 500, timeoutMs: 15_000 });
  await view.waitForMatch(client, { maxCols: size.cols, maxRows: size.rows });
}

/**
 * 2 クライアント (A: 120x40, B: 80x24) の attach / 入力 / detach で、pane サイズの所有者が
 * 最後に入力した側へ移り、所有者が抜けたら残った側に戻ることを確かめる。key は再描画を起こす入力。
 */
export async function runSizeConflict(socket: string, client: MisaoClient, paneId: string, key: string): Promise<void> {
  const a = await ClientView.attach(socket, paneId, 'A', 'snapshot', SIZE_A, { announceSize: true });
  let b: ClientView | undefined;
  try {
    await assertStep(client, paneId, { step: 'A attach', size: SIZE_A, owner: 'A', view: a });
    b = await ClientView.attach(socket, paneId, 'B', 'snapshot', SIZE_B, { announceSize: true });
    await assertStep(client, paneId, { step: 'B attach', size: SIZE_B, owner: 'B', view: b });
    await a.write(key);
    await assertStep(client, paneId, { step: 'A input', size: SIZE_A, owner: 'A', view: a });
    await b.write(key);
    await assertStep(client, paneId, { step: 'B input', size: SIZE_B, owner: 'B', view: b });
    await b.detach();
    b.close();
    await assertStep(client, paneId, { step: 'owner B detach', size: SIZE_A, owner: 'A', view: a });
    assert.equal((await paneInfo(client, paneId)).clients.length, 1, 'A だけが残る');
  } finally {
    a.close();
    b?.close();
  }
}
