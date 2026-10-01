import type { NotificationParams } from '@misao/protocol';

/**
 * 大量に流れる通知 (pane.line / pane.output) の最小構造確認。
 * 完全な Zod 検証は 1 件あたりのコストが大きく、購読者の受信が送り手に追いつかなくなるため、
 * ホットパスでは受け手が実際に使う構造だけを確かめる (ts / paneId の書式は見ない)。
 * 外形 (jsonrpc / method / params) が通知の形でなければ undefined を返し、呼び出し側が完全な検証に回す。
 * 余分なフィールドは保持する (looseObject と同じ)。params は JSON.parse 直後の値なので、replay だけ書き換える。
 */
export type HotNotification =
  | { method: 'pane.line'; params: NotificationParams<'pane.line'> }
  | { method: 'pane.output'; params: NotificationParams<'pane.output'> };

export type HotResult = { ok: true; notification: HotNotification } | { ok: false; message: string };

export function readHotNotification(json: unknown): HotResult | undefined {
  if (typeof json !== 'object' || json === null) return undefined;
  const { jsonrpc, method, params } = json as Record<string, unknown>;
  if (jsonrpc !== '2.0' || 'id' in json) return undefined;
  if (method !== 'pane.line' && method !== 'pane.output') return undefined;
  if (!isPlainObject(params)) return undefined;
  if (!isSeq(params.seq) || typeof params.ts !== 'string' || typeof params.paneId !== 'string') {
    return invalid(method);
  }
  if (method === 'pane.line') {
    if (typeof params.text !== 'string') return invalid(method);
    return { ok: true, notification: { method, params: params as NotificationParams<'pane.line'> } };
  }
  if (typeof params.dataB64 !== 'string') return invalid(method);
  // 知らない replay の種類は 'unknown' として読む (スキーマの catch と同じ)。
  if ('replay' in params && params.replay !== 'snapshot') params.replay = 'unknown';
  return { ok: true, notification: { method, params: params as NotificationParams<'pane.output'> } };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isSeq(value: unknown): boolean {
  return Number.isSafeInteger(value) && (value as number) >= 0; // z.int() と同じく安全な整数に限る
}

function invalid(method: string): HotResult {
  return { ok: false, message: `invalid ${method} params` };
}
