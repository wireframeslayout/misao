import { createHash } from 'node:crypto';
import { userInfo } from 'node:os';
import path from 'node:path';
import { MAX_NAMED_PIPE_NAME_LENGTH, NAMED_PIPE_PREFIX, isNamedPipePath } from '@misao/protocol';

/** sockaddr_un.sun_path は Linux で 108 バイト（終端 NUL を含む）。 */
export const MAX_SOCKET_PATH_BYTES = 107;

const SOCKET_FILE_NAME = 'misao.sock';
const DEFAULT_DIR_NAME = '.misao';
const PIPE_USER_MAX_LENGTH = 32;
const PIPE_HASH_LENGTH = 16;

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
  /** 接続口の種類を決める OS。省略時は実行中の OS。win32 は名前付きパイプ、それ以外は Unix ソケット。 */
  readonly platform?: NodeJS.Platform | undefined;
  /** win32 で既定のパイプ名に入れるユーザー名。省略時は os.userInfo().username。 */
  readonly userName?: string | undefined;
}

function isSet(value: string | undefined): value is string {
  return value !== undefined && value !== '';
}

function pathApi(platform: NodeJS.Platform | undefined): path.PlatformPath {
  return (platform ?? process.platform) === 'win32' ? path.win32 : path.posix;
}

function expandHome(source: string, value: string, homeDir: string, p: path.PlatformPath): string {
  const home = p.sep === '\\' ? ['~/', '~\\'] : ['~/'];
  const expanded =
    value === '~' ? homeDir : home.some((h) => value.startsWith(h)) ? p.join(homeDir, value.slice(2)) : value;
  if (!p.isAbsolute(expanded)) {
    throw new MisaoPathError(`${source} must be an absolute path or start with "~/": ${value}`);
  }
  return expanded;
}

export interface ResolveMisaoDirsInput {
  readonly env: Pick<SocketPathEnv, 'MISAO_DIR'>;
  readonly homeDir: string;
  readonly platform?: NodeJS.Platform | undefined;
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
export function resolveMisaoDirs({ env, homeDir, platform }: ResolveMisaoDirsInput): [MisaoDir, ...MisaoDir[]] {
  const p = pathApi(platform);
  const fallback: MisaoDir = {
    path: p.join(expandHome('home directory', homeDir, homeDir, p), DEFAULT_DIR_NAME),
    origin: 'default',
  };
  if (!isSet(env.MISAO_DIR)) return [fallback];
  return [{ path: expandHome('MISAO_DIR', env.MISAO_DIR, homeDir, p), origin: 'MISAO_DIR' }, fallback];
}

/**
 * win32 の既定のパイプ名: `\\.\pipe\misao-<user>-<hash(dir)>`。
 * ユーザー名で他のユーザーのパイプと名前を分け、ディレクトリのハッシュで $MISAO_DIR ごとに別のデーモンを持てるようにする。
 * 名前は保護にはならない（保護は ACL の役割）。ファイルシステムは大文字小文字を区別しないので、ハッシュ元は小文字にそろえる。
 */
function defaultPipeName(dir: string, userName: string): string {
  const user = userName.replace(/[^A-Za-z0-9._-]/g, '_').slice(0, PIPE_USER_MAX_LENGTH);
  const hash = createHash('sha256').update(dir.toLowerCase()).digest('hex').slice(0, PIPE_HASH_LENGTH);
  return `${NAMED_PIPE_PREFIX}misao-${user}-${hash}`;
}

function requirePipeName(source: string, value: string): string {
  if (!isNamedPipePath(value)) {
    throw new MisaoPathError(
      `${source} must be a named pipe (${NAMED_PIPE_PREFIX}<name>) on Windows: ${value}`,
    );
  }
  return value;
}

function selectWindowsPipeName({ env, explicitPath, homeDir, userName }: ResolveSocketPathInput): string {
  if (isSet(env.MISAO_SOCKET)) return requirePipeName('MISAO_SOCKET', env.MISAO_SOCKET);
  if (explicitPath !== undefined) return requirePipeName('socket path', explicitPath);
  const [preferred] = resolveMisaoDirs({ env, homeDir, platform: 'win32' });
  return defaultPipeName(preferred.path, userName ?? userInfo().username);
}

function selectUnixSocketPath({ env, explicitPath, homeDir }: ResolveSocketPathInput): string {
  const p = path.posix;
  if (isSet(env.MISAO_SOCKET)) return expandHome('MISAO_SOCKET', env.MISAO_SOCKET, homeDir, p);
  if (explicitPath !== undefined) return expandHome('socket path', explicitPath, homeDir, p);
  const [preferred] = resolveMisaoDirs({ env, homeDir, platform: 'linux' });
  return p.join(preferred.path, SOCKET_FILE_NAME);
}

/**
 * 接続口（Unix ソケットのパス、または Windows の名前付きパイプの名前）を解決する関数。
 * 優先順: $MISAO_SOCKET > explicitPath > $MISAO_DIR > ~/.misao。
 * win32 では、どれも名前付きパイプの名前でなければならず、指定が無ければ $MISAO_DIR（既定は ~/.misao）から決まる名前になる。
 * 空文字の環境変数は未設定として扱う。
 */
export function resolveSocketPath(input: ResolveSocketPathInput): string {
  if ((input.platform ?? process.platform) === 'win32') {
    const pipe = selectWindowsPipeName(input);
    if (pipe.length > MAX_NAMED_PIPE_NAME_LENGTH) {
      throw new MisaoPathError(
        `pipe name is ${pipe.length} characters, exceeding the limit of ${MAX_NAMED_PIPE_NAME_LENGTH}: ${pipe}`,
      );
    }
    return pipe;
  }
  const resolved = selectUnixSocketPath(input);
  const bytes = Buffer.byteLength(resolved);
  if (bytes > MAX_SOCKET_PATH_BYTES) {
    throw new MisaoPathError(
      `socket path is ${bytes} bytes, exceeding the limit of ${MAX_SOCKET_PATH_BYTES}: ${resolved}`,
    );
  }
  return resolved;
}
