// NDJSON over Unix socket, JSON-RPC 2.0。1 行 1 JSON。
import { StringDecoder } from 'node:string_decoder';

export const PROTOCOL_VERSION = '0.0.1';

export type JsonValue = unknown;

export interface RpcRequest {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: Record<string, unknown>;
}

export interface RpcError {
  code: number;
  message: string;
}

export interface RpcResponse {
  jsonrpc: '2.0';
  id: number | string | null;
  result?: JsonValue;
  error?: RpcError;
}

/** 通知。seq / ts は params に載る (stream ごとに単調増加)。 */
export interface RpcNotification {
  jsonrpc: '2.0';
  method: string;
  params: { seq: number; ts: string; [key: string]: unknown };
}

export type RpcMessage = RpcRequest | RpcResponse | RpcNotification;

export const ErrorCode = {
  Parse: -32700,
  InvalidRequest: -32600,
  MethodNotFound: -32601,
  InvalidParams: -32602,
  Internal: -32603,
  PaneNotFound: 1001,
  PaneExited: 1002,
  Ambiguous: 1003,
} as const;

export class RpcFailure extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

export function isResponse(m: RpcMessage): m is RpcResponse {
  return 'id' in m && !('method' in m);
}
export function isNotification(m: RpcMessage): m is RpcNotification {
  return 'method' in m && !('id' in m);
}
export function isRequest(m: RpcMessage): m is RpcRequest {
  return 'method' in m && 'id' in m;
}

export function encodeMessage(msg: RpcMessage): string {
  return JSON.stringify(msg) + '\n';
}

/**
 * チャンク境界をまたぐ行分割器。1 チャンクに複数行、1 行が複数チャンクに
 * またがる場合と、UTF-8 マルチバイトがチャンク境界で切れる場合を扱う。
 */
export class LineSplitter {
  private readonly decoder = new StringDecoder('utf8');
  private buf = '';

  push(chunk: Buffer | string): string[] {
    this.buf += typeof chunk === 'string' ? chunk : this.decoder.write(chunk);
    const lines: string[] = [];
    let idx: number;
    while ((idx = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, idx);
      this.buf = this.buf.slice(idx + 1);
      if (line.trim() !== '') lines.push(line);
    }
    return lines;
  }
}

export function nowIso(): string {
  return new Date().toISOString();
}
