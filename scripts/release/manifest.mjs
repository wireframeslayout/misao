// リリース tarball の命名・URL・公開用 package.json を決める純粋関数（I/O なし）。

export const RELEASE_REPO_URL = 'https://github.com/wireframeslayout/misao';

// リリースに含めるワークスペース。protocol を先に置く（sdk が protocol の tarball URL に依存するため）。
export const RELEASE_PACKAGES = [
  { name: '@misao/protocol', dir: 'packages/protocol' },
  { name: '@misao/sdk', dir: 'packages/sdk-node' },
];

// tarball 以外のリリースアセット。CLI + デーモンを 1 ファイルにまとめたバンドル（scripts/bundle-cli.mjs）。
export const RELEASE_ASSETS = [
  { kind: 'bundle', fileName: (version) => bundleName(version) },
  { kind: 'licenses', fileName: (version) => licensesName(version) },
];

export const CHECKSUMS_FILE = 'SHA256SUMS';

// Rust 版の単一バイナリを配布するターゲット (Rust の target triple)。runner は GitHub Actions の実行環境で、
// すべてネイティブにビルドする (クロスコンパイルしない) ので、全ターゲットでスモークテストを実行できる。
// release.yml の matrix と同じ並びにすること (test/release-workflow.test.mjs が一致を確かめる)。
export const BINARY_TARGETS = [
  { target: 'x86_64-unknown-linux-musl', runner: 'ubuntu-24.04' },
  { target: 'aarch64-unknown-linux-musl', runner: 'ubuntu-24.04-arm' },
  { target: 'aarch64-apple-darwin', runner: 'macos-14' },
  { target: 'x86_64-apple-darwin', runner: 'macos-15-intel' },
  { target: 'x86_64-pc-windows-msvc', runner: 'windows-2025' },
];

export function isWindowsTarget(target) {
  return target.includes('-windows-');
}

// アーカイブ内の Rust 版バイナリの名前。
export function binaryExecutableName(target) {
  return isWindowsTarget(target) ? 'misao.exe' : 'misao';
}

// アーカイブのトップレベルのディレクトリ名 (展開すると 1 つのディレクトリになる)。
export function binaryArchiveDir(version, target) {
  return `misao-${version}-${target}`;
}

// misao-<version>-<target>.tar.gz (Windows は .zip)
export function binaryArchiveName(version, target) {
  return `${binaryArchiveDir(version, target)}${isWindowsTarget(target) ? '.zip' : '.tar.gz'}`;
}

// アーカイブのトップレベルのディレクトリの直下に入れるファイル。
export const BINARY_LICENSES_FILE = 'THIRD-PARTY-LICENSES.txt';

export function binaryArchiveMembers(target) {
  return [binaryExecutableName(target), 'LICENSE', 'NOTICE', BINARY_LICENSES_FILE];
}

// Rust 版バイナリのアーカイブ名 (全ターゲット)。
export function binaryFileNames(version) {
  return BINARY_TARGETS.map(({ target }) => binaryArchiveName(version, target));
}

// リリースに載せる全ファイル名 (Node 版の tarball・バンドル + Rust 版のアーカイブ)。SHA256SUMS 自身は含めない。
export function allReleaseFileNames(version) {
  return [...releaseFileNames(version), ...binaryFileNames(version)];
}

// Cargo のワークスペースの version を揃える対象の crate (Cargo.lock の中の名前)。
const CARGO_CRATES = ['misao', 'misao-daemon', 'misao-protocol'];

// Cargo.toml の [workspace.package] の version と、Cargo.lock の自分の crate の version を新しい version にする。
// 置き換え箇所が想定と違う (数が合わない) ときは、黙って進めず例外にする。
export function applyCargoVersion({ cargoToml, cargoLock, version }) {
  const sectionStart = cargoToml.indexOf('[workspace.package]');
  if (sectionStart < 0) throw new Error('Cargo.toml has no [workspace.package] section');
  const nextSection = cargoToml.indexOf('\n[', sectionStart + 1);
  const sectionEnd = nextSection < 0 ? cargoToml.length : nextSection;
  const section = cargoToml.slice(sectionStart, sectionEnd);
  const versionLine = /^version = "[^"]*"$/m;
  if (!versionLine.test(section)) throw new Error('[workspace.package] in Cargo.toml has no version line');
  const nextToml = cargoToml.slice(0, sectionStart) + section.replace(versionLine, `version = "${version}"`) + cargoToml.slice(sectionEnd);

  let replaced = 0;
  const lockPattern = new RegExp(`(\\[\\[package\\]\\]\\nname = "(?:${CARGO_CRATES.join('|')})"\\nversion = ")[^"]*(")`, 'g');
  const nextLock = cargoLock.replace(lockPattern, (_, head, tail) => {
    replaced += 1;
    return `${head}${version}${tail}`;
  });
  if (replaced !== CARGO_CRATES.length) {
    throw new Error(`Cargo.lock: expected ${CARGO_CRATES.length} workspace crates (${CARGO_CRATES.join(', ')}), updated ${replaced}`);
  }
  return { cargoToml: nextToml, cargoLock: nextLock };
}

const PROTOCOL_NAME = '@misao/protocol';
// SemVer 2.0.0 の MAJOR.MINOR.PATCH[-prerelease]（数値に先頭ゼロ不可、build メタデータは扱わない）。
const NUMERIC = '(?:0|[1-9]\\d*)';
const PRERELEASE_ID = `(?:${NUMERIC}|\\d*[A-Za-z-][0-9A-Za-z-]*)`;
const VERSION_PATTERN = new RegExp(`^${NUMERIC}\\.${NUMERIC}\\.${NUMERIC}(?:-${PRERELEASE_ID}(?:\\.${PRERELEASE_ID})*)?$`);

export function parseReleaseVersion(version) {
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`invalid version "${version}": expected MAJOR.MINOR.PATCH[-prerelease]`);
  }
  return { version };
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

// CLI + デーモンのバンドルのファイル名
export function bundleName(version) {
  return `misao-${version}.mjs`;
}

// バンドルに同梱した依存と misao 本体のライセンス全文
export function licensesName(version) {
  return `misao-${version}.LICENSES.txt`;
}

// 公開する全ファイル名（tarball とバンドル）。SHA256SUMS 自身は含めない。
export function releaseFileNames(version) {
  return [
    ...RELEASE_PACKAGES.map(({ name }) => tarballName(name, version)),
    ...RELEASE_ASSETS.map((asset) => asset.fileName(version)),
  ];
}

// sha256sum 互換の行（`<hex>  <file>\n`）。entries は { fileName, sha256 } の配列。
export function formatChecksums(entries) {
  return entries.map(({ fileName, sha256 }) => `${sha256}  ${fileName}\n`).join('');
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
