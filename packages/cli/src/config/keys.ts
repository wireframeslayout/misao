const CTRL_TARGETS = '@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_';
const MIN_PRINTABLE = 0x20;
const MAX_PRINTABLE = 0x7e;

/**
 * キー指定文字列を入力バイトに変換する。
 * `C-<ch>`（ch は `@A-Za-z[\]^_`）は制御コード、単一の印字可能 ASCII はそのまま 1 バイト。
 * `C-[` は ESC になり矢印キー等のエスケープシーケンスと区別できないため拒否する。
 */
export function parseKeySpec(spec: string): number {
  if (spec.startsWith('C-')) return parseControlKey(spec);
  if (spec.length === 1) {
    const code = spec.charCodeAt(0);
    if (code >= MIN_PRINTABLE && code <= MAX_PRINTABLE) return code;
  }
  throw new Error(
    `invalid key spec "${spec}": expected "C-<char>" or a single printable ASCII character`,
  );
}

/**
 * prefix キー用。制御キー（`C-<char>`）のみを許す。
 * 印字可能文字を prefix にすると、その文字を通常の入力として打てなくなるため拒否する。
 */
export function parsePrefixKeySpec(spec: string): number {
  if (!spec.startsWith('C-')) {
    throw new Error(`invalid prefix key "${spec}": prefix must be a control key "C-<char>"`);
  }
  return parseControlKey(spec);
}

function parseControlKey(spec: string): number {
  const target = spec.slice(2).toUpperCase();
  if (target === '[') {
    throw new Error(`invalid key spec "${spec}": C-[ is ESC and cannot be used`);
  }
  if (target.length !== 1 || !CTRL_TARGETS.includes(target)) {
    throw new Error(`invalid key spec "${spec}": C-<char> requires one of ${CTRL_TARGETS}`);
  }
  return target.charCodeAt(0) & 0x1f;
}
