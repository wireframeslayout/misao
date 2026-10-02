// リリース tarball の命名・URL・公開用 package.json を決める純粋関数（I/O なし）。

export const RELEASE_REPO_URL = 'https://github.com/wireframeslayout/misao';

// リリースに含めるワークスペース。protocol を先に置く（sdk が protocol の tarball URL に依存するため）。
export const RELEASE_PACKAGES = [
  { name: '@misao/protocol', dir: 'packages/protocol' },
  { name: '@misao/sdk', dir: 'packages/sdk-node' },
];

const PROTOCOL_NAME = '@misao/protocol';
const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?$/;

export function parseReleaseVersion(version) {
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`invalid version "${version}": expected MAJOR.MINOR.PATCH[-prerelease]`);
  }
  return { version, isPrerelease: version.includes('-') };
}

export function parseReleaseTag(tag) {
  if (!tag.startsWith('v')) {
    throw new Error(`invalid release tag "${tag}": expected v<MAJOR.MINOR.PATCH[-prerelease]>`);
  }
  return { tag, ...parseReleaseVersion(tag.slice(1)) };
}

// "@misao/protocol" -> "misao-protocol-<version>.tgz"（npm pack の出力名と同じ規則）
export function tarballName(packageName, version) {
  return `${packageName.replace(/^@/, '').replace('/', '-')}-${version}.tgz`;
}

export function releaseAssetUrl(tag, fileName) {
  return `${RELEASE_REPO_URL}/releases/download/${tag}/${fileName}`;
}

// ワークスペースの package.json から公開用の package.json を作る。
// dist と型定義のみを公開するため @misao/source 条件 export は外す。
export function buildReleaseManifest({ source, tag, engines }) {
  const { version } = parseReleaseTag(tag);
  if (source.version !== version) {
    throw new Error(
      `${source.name} version is ${source.version} but tag ${tag} requires ${version} (run scripts/set-version.mjs ${version})`,
    );
  }
  const dependencies = Object.fromEntries(
    Object.entries(source.dependencies).map(([name, range]) => [name, rewriteDependency({ name, range, tag, version })]),
  );
  return {
    name: source.name,
    version,
    type: source.type,
    license: source.license,
    main: source.main,
    types: source.types,
    exports: { '.': { types: source.exports['.'].types, default: source.exports['.'].default } },
    files: ['dist', 'LICENSE', 'NOTICE'],
    dependencies,
    engines,
  };
}

function rewriteDependency({ name, range, tag, version }) {
  if (name === PROTOCOL_NAME) return releaseAssetUrl(tag, tarballName(PROTOCOL_NAME, version));
  if (name.startsWith('@misao/')) {
    throw new Error(`unsupported internal dependency ${name}: only ${PROTOCOL_NAME} is released`);
  }
  return range;
}

// 全ワークスペースの version を揃える。内部依存（@misao/*・misao）の参照も新しい version に固定する。
export function applyWorkspaceVersion({ manifest, version, workspaceNames }) {
  const next = { ...manifest, version };
  for (const field of ['dependencies', 'devDependencies']) {
    if (manifest[field] === undefined) continue;
    next[field] = Object.fromEntries(
      Object.entries(manifest[field]).map(([name, range]) => [name, workspaceNames.has(name) ? version : range]),
    );
  }
  return next;
}
