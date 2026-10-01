/** プロファイルが画面を判定するための入力。Pane が xterm から読んで渡す。 */
export interface ProfileScreen {
  /** viewport の各行 (右端の空白は除去済み)。 */
  rows: readonly string[];
  title: string;
  altScreen: boolean;
}

/** null = 意見なし (次の段 title / bytes に任せる)。blocked を出せるのはプロファイルだけ。 */
export type ProfileVerdict = 'working' | 'blocked' | 'idle' | null;

/** エージェント固有の画面規則の差し込み口。実装 (Claude / Codex) は Phase 2。 */
export interface AgentProfile {
  /** 判定したとき pane.state の decidedBy に載る。 */
  readonly name: string;
  matches(cmd: readonly string[]): boolean;
  /** 純粋関数。同じ画面なら同じ結果を返す。 */
  classify(screen: ProfileScreen): ProfileVerdict;
}

/** cmd に最初に matches したプロファイルを返す。無ければ undefined。 */
export function selectProfile(profiles: readonly AgentProfile[], cmd: readonly string[]): AgentProfile | undefined {
  return profiles.find((p) => p.matches(cmd));
}
