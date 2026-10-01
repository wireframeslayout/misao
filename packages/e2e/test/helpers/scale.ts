/** MISAO_E2E_FULL=1 のときだけ spike の規模に戻す。既定は CI で数分に収まる規模。 */
const IS_FULL = process.env.MISAO_E2E_FULL === '1';

export function scale<T>(quick: T, full: T): T {
  return IS_FULL ? full : quick;
}
