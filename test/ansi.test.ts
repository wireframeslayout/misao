import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AnsiLineAssembler } from '../src/util/ansi.js';

function whole(input: string): string[] {
  return new AnsiLineAssembler().push(Buffer.from(input));
}
/** 1 バイトずつ流す。 */
function bytewise(input: string): string[] {
  const a = new AnsiLineAssembler();
  const out: string[] = [];
  for (const b of Buffer.from(input)) out.push(...a.push(Buffer.from([b])));
  return out;
}
/** すべての 2 分割位置で流す。 */
function everySplit(input: string): string[][] {
  const buf = Buffer.from(input);
  const results: string[][] = [];
  for (let i = 0; i <= buf.length; i++) {
    const a = new AnsiLineAssembler();
    results.push([...a.push(buf.subarray(0, i)), ...a.push(buf.subarray(i))]);
  }
  return results;
}

function check(input: string, expected: string[]): void {
  assert.deepEqual(whole(input), expected, 'whole');
  assert.deepEqual(bytewise(input), expected, 'bytewise');
  for (const r of everySplit(input)) assert.deepEqual(r, expected, 'split');
}

test('プレーンテキストと CRLF / LF', () => check('a\r\nb\nc\r\n', ['a', 'b', 'c']));
test('CSI (SGR / カーソル移動) を除去', () => check('\x1b[1;31mred\x1b[0m \x1b[2A\x1b[?25lok\n', ['red ok']));
test('OSC: BEL 終端', () => check('\x1b]0;my title\x07visible\n', ['visible']));
test('OSC: ST (ESC \\) 終端', () => check('\x1b]2;my title\x1b\\visible\n', ['visible']));
test('DCS / APC / PM / SOS は ST まで除去', () => {
  check('\x1bPq#0;2;0;0;0\x1b\\A\x1b_Gdata\x1b\\B\x1b^pm\x1b\\C\x1bXsos\x1b\\D\n', ['ABCD']);
});
test('2 文字 ESC シーケンス (ESC 7 / ESC = / ESC ( B)', () => check('\x1b7a\x1b=b\x1b(Bc\x1b8\n', ['abc']));
test('C0 制御 (BEL, BS, NUL) は捨て、TAB は残す', () => check('a\x07b\x08c\x00\td\n', ['abc\td']));
test('ワイド文字 / 絵文字 / 結合はそのまま', () => check('日本語ＡＢＣ🎉é\n', ['日本語ＡＢＣ🎉é']));
test('CR 上書き: 最後の bare CR 以降を残す', () => {
  check('progress 10%\rprogress 50%\rdone\r\n', ['done']);
});
test('CR + LF は改行 (行は消えない)', () => check('keep\r\nnext\r\n', ['keep', 'next']));
test('CR が連続しても CRLF は改行', () => check('abc\r\r\n', ['abc']));
test('CR の後に SGR だけ来て次の文字で上書き', () => check('old\r\x1b[Knew\n', ['new']));
test('OSC 内の ESC が ST でなければ新しいシーケンスとして解釈', () => check('\x1b]0;t\x1b[31mX\x07Y\n', ['XY']));
test('CAN で sequence を中断', () => check('\x1b[31\x18ok\n', ['ok']));
test('未完了のシーケンスは次の行に漏れない (pending が空)', () => {
  const a = new AnsiLineAssembler();
  assert.deepEqual(a.push('text\x1b]0;unterminated'), []);
  assert.equal(a.pending, 'text');
});
test('マーカーが SGR や分割 write をまたいでも 1 行になる', () => {
  check('\x1b[1;32mAZITO_DO' + 'NE_x_ab' + 'cd\x1b[0m\r\n', ['AZITO_DONE_x_abcd']);
});

test('孤立 ESC の直後の改行でシーケンスを打ち切り、次行のマーカーを取り逃さない', () => {
  check('junk\x1b\nAZITO_DONE_x_1\r\n', ['junk', 'AZITO_DONE_x_1']);
});
test('CSI / ESC 中間バイト途中の CR LF も ground に戻る', () => {
  check('a\x1b[31\r\nAZITO_DONE_x_2\r\n', ['a', 'AZITO_DONE_x_2']);
  check('b\x1b(\nAZITO_DONE_x_3\n', ['b', 'AZITO_DONE_x_3']);
});
test('ESC 直後の範囲外 (非 ASCII) 文字は再解釈され消費されない', () => {
  check('\x1bあいう\n', ['あいう']);
});
