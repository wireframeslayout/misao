import path from 'node:path';
import { resolveMisaoDirs } from '@misao/sdk';
import type { MisaoDirOrigin } from '@misao/sdk';

const CONFIG_FILE_NAME = 'misao.json';

export interface ConfigEnv {
  readonly MISAO_CONFIG?: string | undefined;
  readonly MISAO_DIR?: string | undefined;
  readonly MISAO_SOCKET?: string | undefined;
}

export interface LoadConfigInput {
  readonly flagPath?: string | undefined;
  readonly env: ConfigEnv;
  readonly homeDir: string;
}

/** 候補の指定元。`--config` と `MISAO_CONFIG` は明示指定で、存在しなければエラーにする。 */
export type ConfigOrigin = '--config' | 'MISAO_CONFIG' | MisaoDirOrigin;

export interface ConfigCandidate {
  readonly path: string;
  readonly origin: ConfigOrigin;
}

export function isExplicitOrigin(origin: ConfigOrigin): boolean {
  return origin === '--config' || origin === 'MISAO_CONFIG';
}

/**
 * 設定ファイルの探索候補を優先順に返す: --config > $MISAO_CONFIG > $MISAO_DIR > ~/.misao。
 * 空文字の $MISAO_CONFIG は未設定扱い。ディレクトリの解釈はソケット解決と共通（@misao/sdk）。
 */
export function resolveConfigCandidates({ flagPath, env, homeDir }: LoadConfigInput): ConfigCandidate[] {
  const candidates: ConfigCandidate[] = [];
  if (flagPath !== undefined) candidates.push({ path: flagPath, origin: '--config' });
  if (env.MISAO_CONFIG) candidates.push({ path: env.MISAO_CONFIG, origin: 'MISAO_CONFIG' });
  for (const dir of resolveMisaoDirs({ env, homeDir })) {
    candidates.push({ path: path.join(dir.path, CONFIG_FILE_NAME), origin: dir.origin });
  }
  return candidates;
}
