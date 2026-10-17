// シナリオを Node 版の部品 (xterm + StateTracker) に流して、期待値の記録を作る。
// Pane (packages/daemon/src/pane.ts) の handleData / onTitleChange / screen / apply / handleExit と同じ順序で
// 部品を呼ぶ。時刻は node:test のモックタイマーで進める (実時間を使わないので、何度作っても同じ結果になる)。
import xterm from '@xterm/headless';
import { StateTracker } from '../../packages/daemon/dist/state-tracker.js';
import { viewportText } from '../../packages/daemon/dist/screen.js';
import { selectProfile } from '../../packages/daemon/dist/profile.js';
import { referenceProfiles } from './reference-profiles.mjs';
import { installVirtualClock } from './virtual-clock.mjs';

const START = 1_000_000;
const { Terminal } = xterm;

function bytesOf(step) {
  return step.bytes ? Buffer.from(step.bytes) : Buffer.from(step.data, 'utf8');
}

/** 1 つのシナリオを実行して、画面・タイトル・状態遷移・判定の読み出しを時刻つきの列で返す。 */
export async function runScenario(scenario) {
  const clock = installVirtualClock(START);
  const log = [];
  const elapsed = () => clock.now() - START;
  const advanceTo = (at) => {
    const delta = at - elapsed();
    if (delta < 0) throw new Error(`${scenario.name}: steps must be in time order (at=${at})`);
    clock.tick(delta);
  };
  let term;
  let tracker;
  try {
    term = new Terminal({ cols: scenario.cols, rows: scenario.rows, scrollback: scenario.scrollback, allowProposedApi: true });
    const profile = selectProfile(referenceProfiles, scenario.cmd);
    let title = '';
    let activity = 0;
    tracker = new StateTracker({
      profile,
      readScreen: () => ({ rows: viewportText(term).split('\n'), title, altScreen: term.buffer.active.type === 'alternate' }),
      onChange: (state, decidedBy, prev) => log.push({ kind: 'state', at: elapsed(), state, decidedBy, prev }),
      now: () => Date.now(),
      log: {
        warn: (msg) => {
          throw new Error(`profile failed: ${msg}`);
        },
      },
    });
    term.onTitleChange((t) => {
      title = t;
      tracker.setTitle(t);
      log.push({ kind: 'title', at: elapsed(), title: t });
    });
    for (const step of scenario.steps) {
      advanceTo(step.at);
      switch (step.op) {
        case 'output': {
          const data = bytesOf(step);
          activity++;
          tracker.recordOutput(data.length);
          term.write(data, () => tracker.notifyScreenUpdated());
          clock.tick(0); // xterm の書き込みは setTimeout(0) で進む
          break;
        }
        case 'input':
          tracker.notifyInput();
          break;
        case 'resize':
          activity++;
          tracker.notifyResize();
          term.resize(step.cols, step.rows);
          break;
        case 'screen': {
          const seen = activity;
          const buf = term.buffer.active;
          log.push({
            kind: 'screen',
            at: elapsed(),
            text: viewportText(term),
            cursor: { x: buf.cursorX, y: buf.cursorY },
            altScreen: buf.type === 'alternate',
            title,
            activity: seen,
          });
          break;
        }
        case 'info':
          log.push({ kind: 'info', at: elapsed(), ...tracker.snapshot() });
          break;
        case 'exit':
          tracker.markExited();
          break;
        default:
          throw new Error(`unknown op ${step.op}`);
      }
    }
    advanceTo(scenario.end);
    return { profile: profile?.name ?? null, log };
  } finally {
    tracker?.stop();
    term?.dispose();
    clock.uninstall();
  }
}
