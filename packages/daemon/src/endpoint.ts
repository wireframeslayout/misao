import { lstat, mkdir } from 'node:fs/promises';
import { connect } from 'node:net';
import type { Server } from 'node:net';
import { isNamedPipePath } from '@misao/protocol';
import { ensureSocketDir, listenUnixSocket, removeStaleSocket } from './socket.js';

/**
 * デーモンの接続口。Unix ソケット（POSIX 権限で保護）か Windows の名前付きパイプ（ACL で保護）。
 * どちらで待ち受けるかは OS で決まる。platform は判定を注入するためのもので、
 * 実際に待ち受ける処理（listen / 古い接続口の確認）は実行中の OS の接続口に対してだけ使う。
 */
export type EndpointKind = 'unix' | 'pipe';

/** listen した接続口が、どの仕組みで本人以外から守られているか。 */
export type EndpointProtection =
  /** ソケットファイル 600 + 親ディレクトリ 700 を検証済み。 */
  | 'posix-mode'
  /** プロセスの既定の DACL 任せ。「起動したユーザーのみ」への制限は未実装で、実機での確認が要る（docs/ja/windows.md）。 */
  | 'default-dacl';

export function endpointKind(platform: NodeJS.Platform = process.platform): EndpointKind {
  return platform === 'win32' ? 'pipe' : 'unix';
}

/** 接続口の文字列が、その OS で使える形か検証する。 */
export function assertEndpointPath(endpoint: string, platform: NodeJS.Platform = process.platform): void {
  const kind = endpointKind(platform);
  if (kind === 'pipe' && !isNamedPipePath(endpoint)) {
    throw new Error(`on Windows the daemon endpoint must be a named pipe (\\\\.\\pipe\\<name>): ${endpoint}`);
  }
  if (kind === 'unix' && isNamedPipePath(endpoint)) {
    throw new Error(`a named pipe endpoint is only supported on Windows: ${endpoint}`);
  }
}

/**
 * 待ち受け前の準備と検証。
 * unix: 接続口の親ディレクトリを 700 で用意・検証する（socket.ts）。
 * pipe: 接続口自体は実体を持たないので、pid と状態を置く dataDir だけを用意する。
 */
export async function prepareEndpoint(
  endpoint: string,
  dataDir: string,
  platform: NodeJS.Platform = process.platform,
): Promise<void> {
  assertEndpointPath(endpoint, platform);
  if (endpointKind(platform) === 'unix') return ensureSocketDir(endpoint);
  await mkdir(dataDir, { recursive: true });
  const stat = await lstat(dataDir);
  if (stat.isSymbolicLink()) throw new Error(`data directory ${dataDir} is a symbolic link; use the real path`);
  if (!stat.isDirectory()) throw new Error(`data directory ${dataDir} is not a directory`);
}

/**
 * 古い接続口を片付ける。消したら true。
 * unix: 応答するプロセスの無いソケットファイルを消す。
 * pipe: パイプは作ったプロセスが終われば OS が消すので、消す対象は無い。接続できれば稼働中のデーモンがいる。
 */
export async function removeStaleEndpoint(
  endpoint: string,
  platform: NodeJS.Platform = process.platform,
): Promise<boolean> {
  if (endpointKind(platform) === 'unix') return removeStaleSocket(endpoint);
  const code = await new Promise<string | null>((resolve) => {
    const s = connect(endpoint);
    s.once('connect', () => {
      s.destroy();
      resolve(null);
    });
    s.once('error', (e: NodeJS.ErrnoException) => resolve(e.code ?? 'UNKNOWN'));
  });
  if (code === null) throw new Error(`another daemon is already listening on ${endpoint}`);
  if (code === 'ENOENT') return false;
  throw new Error(`cannot determine whether ${endpoint} is in use (${code})`);
}

/**
 * 接続口で待ち受ける。pipe は他のユーザーへ広げない（readableAll / writableAll を明示的に false）。
 * 保護の仕組みを返すので、呼び出し側は pipe の既定 DACL 任せをログに残せる。
 */
export async function listenEndpoint(
  server: Server,
  endpoint: string,
  platform: NodeJS.Platform = process.platform,
): Promise<EndpointProtection> {
  assertEndpointPath(endpoint, platform);
  if (endpointKind(platform) === 'unix') {
    await listenUnixSocket(server, endpoint);
    return 'posix-mode';
  }
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen({ path: endpoint, readableAll: false, writableAll: false }, () => {
      server.off('error', reject);
      resolve();
    });
  });
  return 'default-dacl';
}

/** 終了時に接続口の実体を消す。unix はソケットファイル。pipe は実体が無いので何もしない。 */
export function endpointFiles(endpoint: string, platform: NodeJS.Platform = process.platform): string[] {
  return endpointKind(platform) === 'unix' ? [endpoint] : [];
}
