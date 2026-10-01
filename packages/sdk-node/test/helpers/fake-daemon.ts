import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { LineSplitter, PROTOCOL_VERSION, encodeMessage } from '@misao/protocol';
import type { RpcMessage } from '@misao/protocol';

export async function waitFor(pred: () => boolean, ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!pred()) {
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

/** 26 文字の ULID 形式の epoch / id 部分。n は 0-9。 */
export function ulid(n: number): string {
  return String(n).padStart(26, '0');
}

export const PANE_ID = `p_${ulid(1)}`;

export interface ReceivedRequest {
  method: string;
  params: Record<string, unknown>;
}

/** 応答。trailing は応答と同じ write で送る通知 (デーモンの afterReply による再生を模す)。 */
export interface Reply {
  result?: unknown;
  error?: { code: number; message: string };
  trailing?: RpcMessage[];
}

type Handler = (params: Record<string, unknown>) => Reply;

export function notification(method: string, params: Record<string, unknown>): RpcMessage {
  return { jsonrpc: '2.0', method, params };
}

export function eventNotification(seq: number): RpcMessage {
  return notification('event', { seq, ts: '2026-01-01T00:00:00.000Z', type: 'test', data: {} });
}

export function lineNotification(seq: number, text: string): RpcMessage {
  return notification('pane.line', { seq, ts: '2026-01-01T00:00:00.000Z', paneId: PANE_ID, text });
}

/** NDJSON / JSON-RPC を話す偽デーモン。応答の差し替え・通知送信・強制切断・再起動(epoch 変更)ができる。 */
export class FakeDaemon {
  readonly dir = fs.mkdtempSync(path.join(os.tmpdir(), 'misao-sdk-'));
  readonly socketPath = path.join(this.dir, 'fake.sock');
  readonly requests: ReceivedRequest[] = [];
  epoch = ulid(1);
  protocolVersion = PROTOCOL_VERSION;
  private server: net.Server | undefined;
  private readonly sockets = new Set<net.Socket>();
  private readonly handlers = new Map<string, Handler>();

  constructor() {
    this.handlers.set('server.info', () => ({
      result: {
        protocolVersion: this.protocolVersion,
        pid: process.pid,
        epoch: this.epoch,
        uptimeSec: 0,
        paneCount: 0,
        eventHead: 0,
      },
    }));
    const subscribe: Handler = () => ({ result: { gap: false, head: 0, epoch: this.epoch } });
    this.handlers.set('events.subscribe', subscribe);
    this.handlers.set('pane.subscribe_lines', subscribe);
  }

  get connectionCount(): number {
    return this.sockets.size;
  }

  received(method: string): ReceivedRequest[] {
    return this.requests.filter((r) => r.method === method);
  }

  handle(method: string, handler: Handler): void {
    this.handlers.set(method, handler);
  }

  async start(): Promise<void> {
    const server = net.createServer((socket) => this.onConnection(socket));
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(this.socketPath, resolve);
    });
    this.server = server;
  }

  /** 全接続へ通知を送る。 */
  notify(...messages: RpcMessage[]): void {
    for (const socket of this.sockets) socket.write(messages.map(encodeMessage).join(''));
  }

  /** 全接続へ生の文字列を書く (不正な行のテスト用)。 */
  writeRaw(text: string): void {
    for (const socket of this.sockets) socket.write(text);
  }

  dropConnections(): void {
    for (const socket of this.sockets) socket.destroy();
  }

  /** 停止して、新しい epoch で同じパスに立ち上げ直す。 */
  async restart(epoch: string): Promise<void> {
    await this.stop();
    this.epoch = epoch;
    await this.start();
  }

  async stop(): Promise<void> {
    this.dropConnections();
    const server = this.server;
    this.server = undefined;
    if (server) await new Promise<void>((resolve) => server.close(() => resolve()));
  }

  async dispose(): Promise<void> {
    await this.stop();
    fs.rmSync(this.dir, { recursive: true, force: true });
  }

  private onConnection(socket: net.Socket): void {
    this.sockets.add(socket);
    const splitter = new LineSplitter();
    socket.on('data', (chunk: Buffer) => {
      for (const line of splitter.push(chunk)) this.onLine(socket, line);
    });
    socket.on('error', () => undefined);
    socket.on('close', () => this.sockets.delete(socket));
  }

  private onLine(socket: net.Socket, line: string): void {
    const request = JSON.parse(line) as { id: number; method: string; params?: Record<string, unknown> };
    const params = request.params ?? {};
    this.requests.push({ method: request.method, params });
    const reply = this.handlers.get(request.method)?.(params) ?? {
      error: { code: -32601, message: `no handler for ${request.method}` },
    };
    const response: RpcMessage = reply.error
      ? { jsonrpc: '2.0', id: request.id, error: reply.error }
      : { jsonrpc: '2.0', id: request.id, result: reply.result };
    socket.write([response, ...(reply.trailing ?? [])].map(encodeMessage).join(''));
  }
}
