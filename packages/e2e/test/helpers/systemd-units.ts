import { spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

const UNIT_DIR = path.join(os.homedir(), '.config/systemd/user');
const DEPLOY_UNIT = new URL('../../../../deploy/misao.service', import.meta.url).pathname;

/** `systemctl --user` を実行して出力 (stdout + stderr) を返す。終了コードは見ない (is-active などは非 0 が正常)。起動できなければ throw する。 */
export function systemctl(...args: string[]): string {
  const result = spawnSync('systemctl', ['--user', ...args], { encoding: 'utf8' });
  if (result.error !== undefined) throw result.error;
  return (result.stdout + result.stderr).trim();
}

export function mainPid(unit: string): number {
  return Number(systemctl('show', '-p', 'MainPID', '--value', unit));
}

/** deploy/misao.service の ExecStart だけを差し替えた unit 本文。ほかの設定 (KillMode / Restart) はそのまま使う。 */
export function deployUnitWithExecStart(execStart: string[]): string {
  const template = fs.readFileSync(DEPLOY_UNIT, 'utf8');
  if (!/^ExecStart=.*$/m.test(template)) throw new Error('deploy/misao.service has no ExecStart');
  return template.replace(/^ExecStart=.*$/m, () => `ExecStart=${execStart.join(' ')}`);
}

/** 一意な unit 名 (他の unit・並行実行と衝突しない)。 */
export function uniqueUnitName(role: string): string {
  return `misao-e2e-${process.pid}-${Date.now().toString(36)}-${role}.service`;
}

export function installUnit(name: string, content: string): void {
  fs.mkdirSync(UNIT_DIR, { recursive: true });
  fs.writeFileSync(path.join(UNIT_DIR, name), content);
  systemctl('daemon-reload');
}

/** unit を止めて、unit ファイルを消す。 */
export function removeUnit(name: string): void {
  systemctl('stop', name);
  fs.rmSync(path.join(UNIT_DIR, name), { force: true });
  systemctl('daemon-reload');
  systemctl('reset-failed', name);
}
