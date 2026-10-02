import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { isPublishedFile, stripSourceMapComment } from '../release/dist-file.mjs';

describe('isPublishedFile', () => {
  it('keeps .js and .d.ts only', () => {
    assert.equal(isPublishedFile('index.js'), true);
    assert.equal(isPublishedFile('index.d.ts'), true);
    assert.equal(isPublishedFile('index.js.map'), false);
    assert.equal(isPublishedFile('index.d.ts.map'), false);
    assert.equal(isPublishedFile('tsconfig.tsbuildinfo'), false);
  });
});

describe('stripSourceMapComment', () => {
  it('removes the trailing sourceMappingURL line without a newline', () => {
    assert.equal(
      stripSourceMapComment("export const a = 1;\n//# sourceMappingURL=index.js.map"),
      'export const a = 1;\n',
    );
  });

  it('removes the sourceMappingURL line followed by a newline', () => {
    assert.equal(
      stripSourceMapComment('export declare const a = 1;\n//# sourceMappingURL=index.d.ts.map\n'),
      'export declare const a = 1;\n',
    );
  });

  it('leaves other content untouched', () => {
    const text = "const s = '//# sourceMappingURL=x';\nexport { s };\n";
    assert.equal(stripSourceMapComment(text), text);
  });
});
