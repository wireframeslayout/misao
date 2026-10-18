import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { forbiddenImports, readPeImports } from '../release/pe-imports.mjs';

// import 表を 1 つのセクション (.idata) に持つ、最小の PE32+ を組み立てる。
function buildPe(dlls) {
  const peOffset = 0x80;
  const optionalSize = 240;
  const sectionTable = peOffset + 24 + optionalSize;
  const rawPointer = 0x400;
  const virtualAddress = 0x1000;
  const descriptors = (dlls.length + 1) * 20;
  const names = dlls.map((d) => `${d}\0`);
  const body = Buffer.alloc(descriptors + names.join('').length);
  let nameAt = descriptors;
  dlls.forEach((dll, i) => {
    body.writeUInt32LE(virtualAddress + nameAt, i * 20 + 12);
    body.write(`${dll}\0`, nameAt, 'latin1');
    nameAt += dll.length + 1;
  });
  const buffer = Buffer.alloc(rawPointer + body.length);
  buffer.writeUInt16LE(0x5a4d, 0);
  buffer.writeUInt32LE(peOffset, 0x3c);
  buffer.writeUInt32LE(0x4550, peOffset);
  buffer.writeUInt16LE(1, peOffset + 6);
  buffer.writeUInt16LE(optionalSize, peOffset + 20);
  buffer.writeUInt16LE(0x20b, peOffset + 24);
  buffer.writeUInt32LE(virtualAddress, peOffset + 24 + 112 + 8);
  buffer.writeUInt32LE(body.length, sectionTable + 8);
  buffer.writeUInt32LE(virtualAddress, sectionTable + 12);
  buffer.writeUInt32LE(body.length, sectionTable + 16);
  buffer.writeUInt32LE(rawPointer, sectionTable + 20);
  body.copy(buffer, rawPointer);
  return buffer;
}

describe('readPeImports', () => {
  it('lists the imported DLL names', () => {
    assert.deepEqual(readPeImports(buildPe(['KERNEL32.dll', 'ntdll.dll', 'VCRUNTIME140.dll'])), ['KERNEL32.dll', 'ntdll.dll', 'VCRUNTIME140.dll']);
  });

  it('rejects data that is not a PE file', () => {
    assert.throws(() => readPeImports(Buffer.alloc(200)), /not a PE file/);
  });
});

describe('forbiddenImports', () => {
  it('flags the Visual C++ and universal C runtime DLLs only', () => {
    const dlls = ['KERNEL32.dll', 'VCRUNTIME140.dll', 'api-ms-win-crt-runtime-l1-1-0.dll', 'ucrtbase.dll', 'api-ms-win-core-synch-l1-2-0.dll', 'bcryptprimitives.dll'];
    assert.deepEqual(forbiddenImports(dlls), ['VCRUNTIME140.dll', 'api-ms-win-crt-runtime-l1-1-0.dll', 'ucrtbase.dll']);
  });
});
