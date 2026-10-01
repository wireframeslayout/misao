import type { RpcMessage } from './jsonrpc.js';

/** NDJSON: 1 行 1 JSON。末尾に改行を付ける。 */
export function encodeMessage(msg: RpcMessage): string {
  return JSON.stringify(msg) + '\n';
}

export const DEFAULT_MAX_LINE_BYTES = 8 * 1024 * 1024;

const LF = 0x0a;

/**
 * チャンク境界をまたぐ行分割器。バイト列のまま行を組み立ててから UTF-8 で
 * 復号するので、マルチバイト文字がチャンク境界で切れても壊れない
 * (LF は UTF-8 のマルチバイト列に現れない)。改行の走査は追加されたチャンクだけで行う。
 * 1 行が maxLineBytes を超えたら例外を投げる (呼び出し側は接続を切る)。
 */
export class LineSplitter {
  private pending: Buffer[] = [];
  private pendingBytes = 0;

  constructor(private readonly maxLineBytes: number = DEFAULT_MAX_LINE_BYTES) {}

  push(chunk: Buffer): string[] {
    const lines: string[] = [];
    let start = 0;
    let idx: number;
    while ((idx = chunk.indexOf(LF, start)) >= 0) {
      this.append(chunk.subarray(start, idx));
      const line = Buffer.concat(this.pending, this.pendingBytes).toString('utf8');
      this.pending = [];
      this.pendingBytes = 0;
      if (line.trim() !== '') lines.push(line);
      start = idx + 1;
    }
    this.append(chunk.subarray(start));
    return lines;
  }

  private append(part: Buffer): void {
    if (part.length === 0) return;
    if (this.pendingBytes + part.length > this.maxLineBytes) {
      this.pending = [];
      this.pendingBytes = 0;
      throw new Error(`NDJSON line exceeds ${this.maxLineBytes} bytes`);
    }
    this.pending.push(part);
    this.pendingBytes += part.length;
  }
}
