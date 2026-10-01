import path from 'node:path';
import { resolveMisaoDir } from '@misao/sdk';

const CONFIG_FILE_NAME = 'misao.json';
const DEFAULT_DIR_NAME = '.misao';

export interface ConfigEnv {
  readonly MISAO_CONFIG?: string | undefined;
  readonly MISAO_DIR?: string | undefined;
  readonly MISAO_SOCKET?: string | undefined;
}

export interface ConfigCandidate {
  readonly path: string;
  /** `--config` / `$MISAO_CONFIG` による指定。存在しなければエラーにする。 */
  readonly isExplicit: boolean;
}

export interface ResolveConfigCandidatesInput {
  readonly flagPath?: string | undefined;
  readonly env: ConfigEnv;
  readonly homeDir: string;
}

function isSet(value: string | undefined): value is string {
  return value !== undefined && value !== '';
}

/**
 * 設定ファイルの探索候補を優先順に返す。空文字の env は未設定扱い。
 * $MISAO_DIR はソケット解決と同じ規則で解釈する（不正なら MisaoPathError）。
 */
export function resolveConfigCandidates({
  flagPath,
  env,
  homeDir,
}: ResolveConfigCandidatesInput): ConfigCandidate[] {
  const candidates: ConfigCandidate[] = [];
  if (flagPath !== undefined) candidates.push({ path: flagPath, isExplicit: true });
  if (isSet(env.MISAO_CONFIG)) candidates.push({ path: env.MISAO_CONFIG, isExplicit: true });
  const misaoDir = resolveMisaoDir({ env, homeDir });
  if (misaoDir !== undefined) {
    candidates.push({ path: path.join(misaoDir, CONFIG_FILE_NAME), isExplicit: false });
  }
  candidates.push({
    path: path.join(homeDir, DEFAULT_DIR_NAME, CONFIG_FILE_NAME),
    isExplicit: false,
  });
  return candidates;
}
