/** 環境変数で opt-in するケースの skip 指定。有効なら undefined (= 実行する)。 */
export function skipUnless(envName: string, reason: string): { skip: string } | { skip?: undefined } {
  return process.env[envName] === '1' ? {} : { skip: `${reason} (${envName}=1 で有効)` };
}
