import type { PaneInfo } from '@misao/protocol';

let counter = 0;

/** 26 文字の ULID 形式 (先頭 0-7) になるよう、連番と固定の飾りから作る。 */
export function fakePaneId(tail: string): string {
  const body = `01M3${'A'.repeat(20 - tail.length)}${tail}`;
  return `p_${body}`.padEnd(28, '0').slice(0, 28);
}

export function makePane(overrides: Partial<PaneInfo> = {}): PaneInfo {
  counter++;
  return {
    paneId: fakePaneId(String(counter).padStart(2, '0')),
    pid: 100 + counter,
    cmd: ['/bin/zsh', '-l'],
    cwd: '/home/test/work',
    workspace: 'default',
    window: { id: 'w_01M3AAAAAAAAAAAAAAAAAAAAAA', name: 'default' },
    labels: {},
    processState: 'running',
    exitCode: null,
    signal: null,
    agentState: 'idle',
    decidedBy: 'bytes',
    title: '',
    lastOutputAt: null,
    cols: 80,
    rows: 24,
    clients: [],
    sizeOwner: null,
    ...overrides,
  };
}
