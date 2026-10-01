import { parseKeySpec } from './config/keys.js';
import { CliError } from './errors.js';

const NAMED_KEYS: Readonly<Record<string, string>> = {
  enter: '\r',
  return: '\r',
  escape: '\x1b',
  esc: '\x1b',
  tab: '\t',
  space: ' ',
  backspace: '\x7f',
  delete: '\x1b[3~',
  up: '\x1b[A',
  down: '\x1b[B',
  right: '\x1b[C',
  left: '\x1b[D',
  home: '\x1b[H',
  end: '\x1b[F',
  pageup: '\x1b[5~',
  pagedown: '\x1b[6~',
};

/** キー名 (Enter / Escape / Tab / C-c / Up など) を端末へ送るバイト列にする。未知の名前は usage エラー。 */
export function encodeKeys(names: readonly string[]): Buffer {
  const parts = names.map((name) => {
    const named = NAMED_KEYS[name.toLowerCase()];
    if (named !== undefined) return Buffer.from(named);
    if (/^c-/i.test(name)) {
      try {
        return Buffer.from([parseKeySpec(name)]);
      } catch {
        throw new CliError('usage', `不明なキー名です: ${name}`);
      }
    }
    throw new CliError('usage', `不明なキー名です: ${name}`);
  });
  return Buffer.concat(parts);
}
