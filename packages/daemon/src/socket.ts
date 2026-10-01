import { chmod, lstat, mkdir } from 'node:fs/promises';
import type { Server } from 'node:net';
import path from 'node:path';

const DIR_MODE = 0o700;
const SOCKET_MODE = 0o600;
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
 * 接続の可否は親ディレクトリ（ensureSocketDir で 700 を検証済み）で守り、
 * listen 後に chmod 0o600 する。umask はプロセス全体（libuv のスレッドプールを含む）に
 * 効くため操作しない。chmod に失敗したら server を閉じてから例外を投げる。
 */
export async function listenUnixSocket(server: Server, socketPath: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    try {
      server.listen(socketPath, () => {
        server.off('error', reject);
        resolve();
      });
    } catch (error) {
      server.off('error', reject);
      throw error;
    }
  });
  try {
    await chmod(socketPath, SOCKET_MODE);
  } catch (error) {
    server.close();
    throw error;
  }
}
