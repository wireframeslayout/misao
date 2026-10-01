import { StringDecoder } from 'node:string_decoder';
import type { RpcMessage } from './jsonrpc.js';

/** NDJSON: 1 行 1 JSON。末尾に改行を付ける。 */
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
