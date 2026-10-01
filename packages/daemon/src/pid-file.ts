import * as fs from 'node:fs';

/** pid のプロセスが生きているか。EPERM は「存在するが権限がない」なので生存扱い。 */
function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/**
 * pid ファイルを O_EXCL で作り、起動を排他にする。
 * 既存のファイルの pid が生きていれば例外。死んでいる (または中身が pid でない) なら置き換える。
 */
export function acquirePidFile(pidPath: string, pid: number): void {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      fs.writeFileSync(pidPath, String(pid), { flag: 'wx' });
      return;
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== 'EEXIST') throw e;
    }
    const owner = Number.parseInt(fs.readFileSync(pidPath, 'utf8'), 10);
    if (Number.isInteger(owner) && owner > 0 && isAlive(owner)) {
      throw new Error(`another daemon (pid ${owner}) holds ${pidPath}`);
    }
    fs.rmSync(pidPath, { force: true });
  }
  throw new Error(`could not acquire ${pidPath}: another process keeps recreating it`);
}
