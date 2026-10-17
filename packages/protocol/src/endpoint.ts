/** Windows の名前付きパイプの接頭辞。`\\.\pipe\<name>`。 */
export const NAMED_PIPE_PREFIX = '\\\\.\\pipe\\';

/** CreateNamedPipe の名前の上限（接頭辞を含めて 256 文字）。 */
export const MAX_NAMED_PIPE_NAME_LENGTH = 256;

/** 接続口が Windows の名前付きパイプの名前か。接頭辞は大文字小文字を区別せず、`/` 区切りも同じ扱いにする。 */
export function isNamedPipePath(value: string): boolean {
  return value.slice(0, NAMED_PIPE_PREFIX.length).replaceAll('/', '\\').toLowerCase() === NAMED_PIPE_PREFIX.toLowerCase();
}
