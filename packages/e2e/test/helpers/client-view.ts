import xterm from '@xterm/headless';
import { viewportText } from '@misao/daemon';
import { MisaoClient } from '@misao/sdk';

export type Replay = 'raw' | 'snapshot' | 'none';

export interface ScreenComparison {
  rows: number;
  matchedRows: number;
  matchRate: number;
  mismatches: Array<{ row: number; view: string; daemon: string }>;
}

/** 1 クライアントの視点: 受信した pane.output を自サイズの headless 端末に流して画面を再構成する。 */
export class ClientView {
  readonly term: InstanceType<typeof xterm.Terminal>;
  bytes = 0;
  private chain: Promise<void> = Promise.resolve();

  private constructor(
    readonly client: MisaoClient,
    readonly clientId: string,
    readonly paneId: string,
    readonly cols: number,
    readonly rows: number,
  ) {
    this.term = new xterm.Terminal({ cols, rows, scrollback: 1000, allowProposedApi: true });
    client.onNotification((n) => {
      if (n.method !== 'pane.output' || n.params.paneId !== paneId) return;
      const data = Buffer.from(n.params.dataB64, 'base64');
      this.bytes += data.length;
      this.chain = this.chain.then(() => new Promise<void>((resolve) => this.term.write(data, resolve)));
    });
  }

  /** 新しい接続で pane に attach する。size を announce するとデーモンの pane サイズ調停に加わる。 */
  static async attach(
    socketPath: string,
    paneId: string,
    clientId: string,
    replay: Replay,
    size: { cols: number; rows: number },
    { announceSize = false }: { announceSize?: boolean } = {},
  ): Promise<ClientView> {
    const client = new MisaoClient({ socketPath });
    await client.connect();
    const view = new ClientView(client, clientId, paneId, size.cols, size.rows);
    await client.request('pane.attach', { paneId, clientId, replay, ...(announceSize ? size : {}) });
    return view;
  }

  async text(): Promise<string> {
    await this.chain;
    await new Promise<void>((resolve) => this.term.write('', resolve));
    return viewportText(this.term);
  }

  async write(data: string): Promise<void> {
    await this.client.request('pane.write', { paneId: this.paneId, data, source: 'terminal', clientId: this.clientId });
  }

  async detach(): Promise<void> {
    await this.client.request('pane.detach', { paneId: this.paneId });
  }

  close(): void {
    this.client.close();
  }
}

/** 行ごとに末尾空白を無視して比較する。maxCols / maxRows を渡すとその範囲だけ見る。 */
export function compareScreens(viewText: string, daemonText: string, maxCols?: number, maxRows?: number): ScreenComparison {
  const normalize = (line: string): string => (maxCols === undefined ? line : line.slice(0, maxCols)).replace(/\s+$/, '');
  const view = viewText.split('\n');
  const daemon = daemonText.split('\n');
  const rows = Math.min(maxRows ?? Infinity, Math.max(view.length, daemon.length));
  const mismatches: ScreenComparison['mismatches'] = [];
  let matchedRows = 0;
  for (let row = 0; row < rows; row++) {
    const a = normalize(view[row] ?? '');
    const b = normalize(daemon[row] ?? '');
    if (a === b) matchedRows++;
    else if (mismatches.length < 10) mismatches.push({ row, view: a, daemon: b });
  }
  return { rows, matchedRows, matchRate: rows === 0 ? 1 : matchedRows / rows, mismatches };
}
