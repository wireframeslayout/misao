// 公開 tarball に入れる dist ファイルの選別と内容の整形（I/O なし）。

// 型定義と実行コードだけを残す（ソースマップ・tsbuildinfo・.d.ts.map は含めない）。
export function isPublishedFile(fileName) {
  return fileName.endsWith('.d.ts') || fileName.endsWith('.js');
}

const SOURCE_MAP_COMMENT = /^\/\/# sourceMappingURL=.*$\n?/gm;

// .map は同梱しないので、それを指す sourceMappingURL コメント行を取り除く。
export function stripSourceMapComment(text) {
  return text.replace(SOURCE_MAP_COMMENT, '');
}
