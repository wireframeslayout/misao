import * as fs from 'node:fs';
import type { MisaoClient } from '@misao/sdk';
import type { PaneInfo } from '@misao/protocol';
import { nodeCmd } from './node-cmd.js';
import { sleep, waitFor } from './wait.js';

const FAKE_AGENT = new URL('../../src/fixtures/fake-agent.ts', import.meta.url).pathname;

export const REPO_ROOT = new URL('../../../..', import.meta.url).pathname;

export const MARKER_RE = /AZITO_DONE_[A-Za-z0-9]+_[A-Za-z0-9]+/g;

/** fake-agent を pane で動かすための cmd。 */
export function fakeAgentCmd(...args: string[]): string[] {
  return nodeCmd(FAKE_AGENT, ...args);
}

export async function openPane(
  client: MisaoClient,
  cmd: string[],
  extra: { cols?: number; rows?: number; cwd?: string } = {},
): Promise<string> {
  const { paneId } = await client.request('pane.open', { cmd, cols: 100, rows: 30, ...extra });
  return paneId;
}

export async function paneInfo(client: MisaoClient, paneId: string): Promise<PaneInfo> {
  return client.request('pane.info', { paneId });
}

export async function screenText(client: MisaoClient, paneId: string): Promise<string> {
  return (await client.request('pane.screen', { paneId })).text;
}

/** hub (制御アプリ) からの入力として pane へ書く。 */
export async function typeKeys(client: MisaoClient, paneId: string, data: string): Promise<void> {
  await client.request('pane.write', { paneId, data, source: 'hub' });
}

export async function waitForExit(client: MisaoClient, paneId: string, timeoutMs: number): Promise<PaneInfo> {
  await waitFor(async () => (await paneInfo(client, paneId)).processState === 'exited', `pane ${paneId} to exit`, { timeoutMs, stepMs: 200 });
  return paneInfo(client, paneId);
}

/** fake-agent markers が --out に書く、発行した nonce の一覧。 */
export function readAgentSummary(file: string): { emitted: number; nonces: string[] } {
  const text = fs.readFileSync(file, 'utf8').replace(/^FAKE_SUMMARY /, '');
  return JSON.parse(text) as { emitted: number; nonces: string[] };
}

/** claude / codex を pane で動かすとき、親の CLAUDE* / CODEX* / ANTHROPIC_* を引き継がない cmd 接頭辞。 */
export function cleanEnvPrefix(): string[] {
  const names = Object.keys(process.env).filter((k) => /^(CLAUDE|CODEX|ANTHROPIC_)/.test(k) && k !== 'ANTHROPIC_API_KEY');
  return names.length > 0 ? ['env', ...names.flatMap((n) => ['-u', n])] : [];
}

/** Claude Code の「フォルダを信頼するか」ダイアログを、Yes を選んで通過する。出なければ何もしない。 */
export async function passTrustDialog(client: MisaoClient, paneId: string, maxMs = 20_000): Promise<void> {
  const start = Date.now();
  let handled = false;
  while (Date.now() - start < maxMs) {
    const text = await screenText(client, paneId);
    if (/trust this folder/i.test(text)) {
      const selected = text.split('\n').find((line) => line.includes('❯')) ?? '';
      await typeKeys(client, paneId, /Yes/i.test(selected) ? '\r' : '\x1b[B');
      await sleep(500);
      if (!/Yes/i.test(selected)) await typeKeys(client, paneId, '\r');
      await sleep(1500);
      handled = true;
      continue;
    }
    if (handled || text.trim().length > 40) return;
    await sleep(500);
  }
}
