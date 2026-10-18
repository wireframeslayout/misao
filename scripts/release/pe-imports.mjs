// Windows の実行ファイル (PE) の import 表を読み、依存している DLL の名前を返す純粋関数 (I/O なし)。
// 配布する misao.exe が Visual C++ ランタイムなどに依存していないことを確かめるのに使う。

// 静的リンクした C ランタイムの代わりに、別途インストールが必要になる DLL。
const FORBIDDEN_IMPORT = /^(?:vcruntime|msvcp|ucrtbase|vcomp|concrt)|^api-ms-win-crt-/i;

export function forbiddenImports(dlls) {
  return dlls.filter((name) => FORBIDDEN_IMPORT.test(name));
}

export function readPeImports(buffer) {
  if (buffer.length < 0x40 || buffer.readUInt16LE(0) !== 0x5a4d) throw new Error('not a PE file (no MZ header)');
  const pe = buffer.readUInt32LE(0x3c);
  if (buffer.readUInt32LE(pe) !== 0x00004550) throw new Error('not a PE file (no PE signature)');
  const sectionCount = buffer.readUInt16LE(pe + 6);
  const optionalSize = buffer.readUInt16LE(pe + 20);
  const optional = pe + 24;
  const magic = buffer.readUInt16LE(optional);
  if (magic !== 0x20b && magic !== 0x10b) throw new Error(`unknown optional header magic 0x${magic.toString(16)}`);
  // データディレクトリは PE32+ が +112、PE32 が +96。[1] が import 表。
  const importDirectory = optional + (magic === 0x20b ? 112 : 96) + 8;
  const importRva = buffer.readUInt32LE(importDirectory);
  if (importRva === 0) return [];

  const sections = Array.from({ length: sectionCount }, (_, i) => {
    const at = optional + optionalSize + i * 40;
    return {
      virtualSize: buffer.readUInt32LE(at + 8),
      virtualAddress: buffer.readUInt32LE(at + 12),
      rawSize: buffer.readUInt32LE(at + 16),
      rawPointer: buffer.readUInt32LE(at + 20),
    };
  });
  const toOffset = (rva) => {
    const section = sections.find((s) => rva >= s.virtualAddress && rva < s.virtualAddress + Math.max(s.virtualSize, s.rawSize));
    if (section === undefined) throw new Error(`RVA 0x${rva.toString(16)} is in no section`);
    return section.rawPointer + (rva - section.virtualAddress);
  };

  const names = [];
  for (let at = toOffset(importRva); buffer.readUInt32LE(at + 12) !== 0; at += 20) {
    const nameAt = toOffset(buffer.readUInt32LE(at + 12));
    names.push(buffer.toString('latin1', nameAt, buffer.indexOf(0, nameAt)));
  }
  return names;
}
