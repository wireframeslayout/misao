import { chmod, lstat, mkdir } from 'node:fs/promises';
import type { Server } from 'node:net';
import path from 'node:path';

const DIR_MODE = 0o700;
const SOCKET_MODE = 0o600;
const SOCKET_UMASK = 0o177;
const GROUP_OTHER_BITS = 0o077;

/**
 * ソケットの親ディレクトリを mode 700 で用意する。
 * 既存の場合は、ディレクトリであること・所有者が現在の uid であること・
 * group/other に権限が無いことを検証し、違反はエラーにする（修正はしない）。
 */
export async function ensureSocketDir(socketPath: string): Promise<void> {
  const dir = path.dirname(socketPath);
  await mkdir(dir, { recursive: true, mode: DIR_MODE });
  const stat = await lstat(dir);
  if (stat.isSymbolicLink()) {
    throw new Error(`socket directory ${dir} is a symbolic link; use the real path`);
  }
  if (!stat.isDirectory()) {
    throw new Error(`socket directory ${dir} is not a directory`);
  }
  if (stat.uid !== process.getuid?.()) {
    throw new Error(`socket directory ${dir} is not owned by the current user`);
  }
  if ((stat.mode & GROUP_OTHER_BITS) !== 0) {
    throw new Error(
      `socket directory ${dir} is accessible by group/other (mode ${(stat.mode & 0o777).toString(8)}); run: chmod 700 ${dir}`,
    );
  }
}

/**
 * Unix ソケットで listen し、ソケットファイルを mode 600 にする。
 *
 * bind 時の権限を確定させるため `process.umask(0o177)` の下で listen する。
 * umask はプロセス全体に効くため、listen 呼び出しの間（同期区間）だけ設定し、
 * try/finally で必ず元に戻す。その後、念のため chmod 0o600 を行う。
 */
export async function listenUnixSocket(server: Server, socketPath: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    const previousUmask = process.umask(SOCKET_UMASK);
    try {
      server.listen(socketPath, () => {
        server.off('error', reject);
        resolve();
      });
    } catch (error) {
      server.off('error', reject);
      throw error;
    } finally {
      process.umask(previousUmask);
    }
  });
  await chmod(socketPath, SOCKET_MODE);
}
