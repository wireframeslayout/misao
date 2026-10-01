import { readFileSync } from 'node:fs';
import { MisaoPathError, resolveSocketPath } from '@misao/sdk';
import type { z } from 'zod';
import { resolveConfigCandidates } from './paths.js';
import type { ConfigEnv } from './paths.js';
import { MisaoConfigSchema } from './schema.js';
import type { MisaoConfig } from './schema.js';

export class ConfigError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = 'ConfigError';
  }
}

export type ResolvedConfig = Omit<MisaoConfig, 'socket'> & { readonly socket: string };

export interface LoadConfigInput {
  readonly flagPath?: string | undefined;
  readonly env: ConfigEnv;
  readonly homeDir: string;
}

export interface LoadedConfig {
  readonly config: ResolvedConfig;
  /** 読み込んだ設定ファイル。見つからず既定値で動く場合は null。 */
  readonly source: string | null;
  readonly warnings: string[];
}

interface ReadResult {
  readonly source: string;
  readonly text: string;
}

function isErrnoException(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function formatPath(path: PropertyKey[]): string {
  return path.length === 0 ? '(root)' : path.map(String).join('.');
}

/** パス設定の不正（MisaoPathError）を ConfigError に包み直す。それ以外の例外はそのまま投げる。 */
function withPathErrorAsConfigError<T>(resolve: () => T, source: string | null): T {
  try {
    return resolve();
  } catch (error) {
    if (!(error instanceof MisaoPathError)) throw error;
    const context = source === null ? '' : ` (config: ${source})`;
    throw new ConfigError(`invalid path setting: ${error.message}${context}`, { cause: error });
  }
}

function readFirstConfig(input: LoadConfigInput): ReadResult | null {
  const candidates = withPathErrorAsConfigError(() => resolveConfigCandidates(input), null);
  for (const candidate of candidates) {
    try {
      return { source: candidate.path, text: readFileSync(candidate.path, 'utf8') };
    } catch (error) {
      if (isErrnoException(error) && error.code === 'ENOENT' && !candidate.isExplicit) continue;
      throw new ConfigError(`cannot read config file ${candidate.path}: ${String(error)}`, {
        cause: error,
      });
    }
  }
  return null;
}

function parseJson({ source, text }: ReadResult): unknown {
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new ConfigError(`invalid JSON in ${source}: ${String(error)}`, { cause: error });
  }
}

/** issue が指す未知キーを取り除いたコピーを返す。 */
function stripUnrecognizedKeys(raw: unknown, issues: z.core.$ZodIssueUnrecognizedKeys[]): unknown {
  const copy = structuredClone(raw);
  for (const issue of issues) {
    let target: unknown = copy;
    for (const segment of issue.path) {
      target = isRecord(target) ? target[String(segment)] : undefined;
    }
    if (!isRecord(target)) continue;
    for (const key of issue.keys) delete target[key];
  }
  return copy;
}

function formatIssues(source: string, issues: z.core.$ZodIssue[]): string {
  const lines = issues.map((issue) => `  ${formatPath(issue.path)}: ${issue.message}`);
  return `invalid config ${source}:\n${lines.join('\n')}`;
}

function validateConfig(
  raw: unknown,
  source: string,
): { config: MisaoConfig; warnings: string[] } {
  const first = MisaoConfigSchema.safeParse(raw);
  if (first.success) return { config: first.data, warnings: [] };

  const unrecognized = first.error.issues.filter((i) => i.code === 'unrecognized_keys');
  if (unrecognized.length !== first.error.issues.length) {
    throw new ConfigError(formatIssues(source, first.error.issues), { cause: first.error });
  }
  const second = MisaoConfigSchema.safeParse(stripUnrecognizedKeys(raw, unrecognized));
  if (!second.success) {
    throw new ConfigError(formatIssues(source, second.error.issues), { cause: second.error });
  }
  const warnings = unrecognized.flatMap((issue) =>
    issue.keys.map((key) => `${source}: unknown key "${formatPath([...issue.path, key])}" ignored`),
  );
  return { config: second.data, warnings };
}

/**
 * 設定ファイルを探索・検証し、ソケットパスを解決した設定を返す。
 * 最初に見つかった 1 ファイルだけを使う（マージしない）。副作用は持たず、
 * 警告は戻り値、失敗は ConfigError で伝える。
 */
export function loadConfig(input: LoadConfigInput): LoadedConfig {
  const found = readFirstConfig(input);
  const { config, warnings } =
    found === null
      ? { config: MisaoConfigSchema.parse({}), warnings: [] }
      : validateConfig(parseJson(found), found.source);
  const source = found?.source ?? null;
  const { socket: _configSocket, ...rest } = config;
  const socket = withPathErrorAsConfigError(
    () => resolveSocketPath({ env: input.env, explicitPath: config.socket, homeDir: input.homeDir }),
    source,
  );
  return { config: { ...rest, socket }, source, warnings };
}
