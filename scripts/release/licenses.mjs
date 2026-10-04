// バンドルに含めた依存のライセンス表記を組み立てる純粋関数（I/O なし）。

const NODE_MODULES_PACKAGE = /(?<=^|\/)node_modules\/((?:@[^/]+\/)?[^/]+)\//;

// esbuild の metafile の入力パスから、node_modules 配下のパッケージのディレクトリを列挙する（重複なし、名前順）。
// 入力は metafile の inputs のキー（cwd からの相対パス）。ワークスペースのソース（packages/...）は含めない。
export function listBundledPackageDirs(inputPaths) {
  const dirs = new Set();
  for (const input of inputPaths) {
    const normalized = input.replaceAll('\\', '/');
    let last;
    for (const match of normalized.matchAll(new RegExp(NODE_MODULES_PACKAGE, 'g'))) last = match;
    if (last === undefined) continue;
    dirs.add(normalized.slice(0, last.index + last[0].length - 1));
  }
  return [...dirs].sort();
}

const RULE = '='.repeat(72);

// packages: { name, version, license, text }[]。misao 本体の LICENSE / NOTICE を先頭に置く。
export function formatLicenses({ version, misaoLicense, misaoNotice, packages }) {
  const sections = [
    `misao ${version}\n\nThis file accompanies misao-${version}.mjs. It contains the license of misao itself and\nthe licenses of the third-party packages bundled into that file.`,
    `${RULE}\nmisao (Apache-2.0)\n${RULE}\n\n${misaoLicense.trim()}\n\n--- NOTICE ---\n\n${misaoNotice.trim()}`,
    ...packages.map(
      ({ name, version: v, license, text }) => `${RULE}\n${name}@${v} (${license})\n${RULE}\n\n${text.trim()}`,
    ),
  ];
  return `${sections.join('\n\n')}\n`;
}
