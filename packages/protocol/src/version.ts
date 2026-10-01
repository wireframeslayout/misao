/**
 * プロトコルバージョン (semver)。
 *
 * 互換ポリシー:
 * - フィールド・メソッド・イベント種別・enum 値の追加は minor で、後方互換。
 * - 受信側 (result / 通知 / イベント data) は未知のフィールドと未知の enum 値を許容する。
 *   未知の enum 値は "unknown" として読む。送信側 (params) は厳密に検証する。
 * - 削除や意味の変更は major。
 */
export const PROTOCOL_VERSION = '0.1.0';

function parseMajor(version: string): string {
  const major = version.split('.')[0];
  if (major === undefined || !/^\d+$/.test(major)) {
    throw new Error(`invalid protocol version: ${version}`);
  }
  return major;
}

/** major が一致すれば互換。 */
export function isCompatibleProtocolVersion(server: string, client: string): boolean {
  return parseMajor(server) === parseMajor(client);
}
