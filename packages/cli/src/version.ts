import { readFileSync } from 'node:fs';

/** リリースのバンドルでは esbuild の define (scripts/bundle-cli.mjs) が埋め込む。 */
declare const __MISAO_VERSION__: string | undefined;

function readPackageVersion(): string {
  const manifest: unknown = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  if (typeof manifest !== 'object' || manifest === null || !('version' in manifest) || typeof manifest.version !== 'string') {
    throw new Error('packages/cli/package.json has no version');
  }
  return manifest.version;
}

/** この CLI / デーモンのビルド版。バンドルでは埋め込み値、tsc ビルドと開発時は package.json (set-version と同じ値)。 */
export const MISAO_VERSION: string = typeof __MISAO_VERSION__ === 'string' ? __MISAO_VERSION__ : readPackageVersion();
