import path from 'node:path';

/** sockaddr_un.sun_path は Linux で 108 バイト（終端 NUL を含む）。 */
export const MAX_SOCKET_PATH_BYTES = 107;

const SOCKET_FILE_NAME = 'misao.sock';
const DEFAULT_DIR_NAME = '.misao';

/** misao のパス設定（$MISAO_SOCKET / $MISAO_DIR / 明示指定）が不正。 */
export class MisaoPathError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'MisaoPathError';
  }
}

export interface SocketPathEnv {
  readonly MISAO_SOCKET?: string | undefined;
  readonly MISAO_DIR?: string | undefined;
}

export interface ResolveSocketPathInput {
  readonly env: SocketPathEnv;
  /** misao.json の `socket` など、呼び出し側が明示したパス。 */
  readonly explicitPath?: string | undefined;
  readonly homeDir: string;
}

function isSet(value: string | undefined): value is string {
  return value !== undefined && value !== '';
}

function expandHome(source: string, value: string, homeDir: string): string {
  const expanded = value === '~' ? homeDir : value.startsWith('~/') ? path.join(homeDir, value.slice(2)) : value;
  if (!path.isAbsolute(expanded)) {
    throw new MisaoPathError(`${source} must be an absolute path or start with "~/": ${value}`);
  }
  return expanded;
}

export interface ResolveMisaoDirsInput {
  readonly env: Pick<SocketPathEnv, 'MISAO_DIR'>;
  readonly homeDir: string;
}

export type MisaoDirOrigin = 'MISAO_DIR' | 'default';

export interface MisaoDir {
  readonly path: string;
  readonly origin: MisaoDirOrigin;
}

/**
 * misao のディレクトリ候補を優先順に返す: $MISAO_DIR（設定時のみ）> ~/.misao。
 * $MISAO_DIR は `~/` 展開した絶対パスとし、空文字は未設定扱い。
 * 設定ファイル探索とソケット解決で同じ解釈を使うため、ここに一本化する。
 */
export function resolveMisaoDirs({ env, homeDir }: ResolveMisaoDirsInput): [MisaoDir, ...MisaoDir[]] {
  const fallback: MisaoDir = {
    path: path.join(expandHome('home directory', homeDir, homeDir), DEFAULT_DIR_NAME),
    origin: 'default',
  };
  if (!isSet(env.MISAO_DIR)) return [fallback];
  return [{ path: expandHome('MISAO_DIR', env.MISAO_DIR, homeDir), origin: 'MISAO_DIR' }, fallback];
}

function selectSocketPath({ env, explicitPath, homeDir }: ResolveSocketPathInput): string {
  if (isSet(env.MISAO_SOCKET)) return expandHome('MISAO_SOCKET', env.MISAO_SOCKET, homeDir);
  if (explicitPath !== undefined) return expandHome('socket path', explicitPath, homeDir);
  const [preferred] = resolveMisaoDirs({ env, homeDir });
  return path.join(preferred.path, SOCKET_FILE_NAME);
}

/**
 * ソケットパスを解決する純関数。
 * 優先順: $MISAO_SOCKET > explicitPath > $MISAO_DIR/misao.sock > ~/.misao/misao.sock。
 * 空文字の環境変数は未設定として扱う。
 */
export function resolveSocketPath(input: ResolveSocketPathInput): string {
  const resolved = selectSocketPath(input);
  const bytes = Buffer.byteLength(resolved);
  if (bytes > MAX_SOCKET_PATH_BYTES) {
    throw new MisaoPathError(
      `socket path is ${bytes} bytes, exceeding the limit of ${MAX_SOCKET_PATH_BYTES}: ${resolved}`,
    );
  }
  return resolved;
}
