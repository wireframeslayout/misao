/** デーモンのドメインモデル。永続化 (persistence.ts) はこの型に合わせる。 */

export interface WindowDef {
  id: string;
  name: string;
}

export interface WorkspaceDef {
  name: string;
  windows: WindowDef[];
}

/** pane のメタ (persistence.json に保存するもの)。env は pane.open の env 分のみで、ephemeralEnv は持たない。 */
export interface PaneRecord {
  paneId: string;
  windowId: string;
  cmd: string[];
  cwd: string;
  env: Record<string, string>;
  labels: Record<string, string>;
  cols: number;
  rows: number;
}
