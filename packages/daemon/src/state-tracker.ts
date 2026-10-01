import type { PaneInfo } from '@misao/protocol';
import { ByteActivity } from './byte-activity.js';
import type { Logger } from './log.js';
import type { AgentProfile, ProfileScreen, ProfileVerdict } from './profile.js';
import { TitleActivity } from './title-activity.js';

export type AgentState = PaneInfo['agentState'];

/** bytes 段の評価と、title 段の更新の古さの判定を行う間隔。 */
const TICK_MS = 1000;
/** プロファイルの画面判定は、出力が落ち着くまでこの時間待つ。 */
const PROFILE_DEBOUNCE_MS = 120;
/** 出力が続いても、最初の出力からこの時間以内には判定する。 */
const PROFILE_MAX_WAIT_MS = 300;
/** classify がこの回数続けて例外を投げたら、その pane ではプロファイルを使わない。 */
const PROFILE_MAX_FAILURES = 3;

export interface StateTrackerOptions {
  /** この pane に使うプロファイル。無ければ title / bytes 段だけで判定する。 */
  profile?: AgentProfile;
  /** プロファイル判定のための現在の画面。profile があるときだけ、notifyScreenUpdated の後に呼ぶ。 */
  readScreen: () => ProfileScreen;
  /** 状態または decidedBy が変わるたびに呼ぶ。 */
  onChange: (state: AgentState, decidedBy: string, prev: AgentState) => void;
  now: () => number;
  /** プロファイルの例外など、判定を続けながら残すべき出来事。 */
  log: Pick<Logger, 'warn'>;
}

/**
 * 稼働判定のコア。exit > profile > title > bytes の順に見て、最初に意見を持った段で決める。
 * コアは blocked を出さない (blocked を出せるのはプロファイルだけ)。
 * 1 秒ごとのタイマーは markExited / stop で止まる。
 */
export class StateTracker {
  private state: AgentState = 'unknown';
  private decidedBy = 'none';
  private isExited = false;
  /** stop 後は、遅れて届く画面更新の通知でタイマーを作り直さない。 */
  private isStopped = false;
  private profileVerdict: ProfileVerdict = null;
  private readonly bytes: ByteActivity;
  private readonly title = new TitleActivity();
  private byteVerdict: ReturnType<ByteActivity['tick']> = null;
  private readonly tickTimer: NodeJS.Timeout;
  private profileTimer: NodeJS.Timeout | undefined;
  private profileWaitSince: number | undefined;
  /** 連続して失敗したら undefined にする (以後は予約しない)。 */
  private profile: AgentProfile | undefined;
  private profileFailures = 0;

  constructor(private readonly opts: StateTrackerOptions) {
    this.profile = opts.profile;
    this.bytes = new ByteActivity(opts.now());
    this.tickTimer = setInterval(() => this.tick(), TICK_MS);
    this.tickTimer.unref();
  }

  snapshot(): { agentState: AgentState; decidedBy: string } {
    return { agentState: this.state, decidedBy: this.decidedBy };
  }

  recordOutput(bytes: number): void {
    if (this.isExited) return;
    this.bytes.record(bytes, this.opts.now());
  }

  /**
   * 画面モデル (xterm) が出力を解析し終えたときに呼ぶ。プロファイル判定はここから予約する。
   * 出力の受信時に予約すると、解析前の古い画面を読んでしまう (最後のチャンクを取り逃す)。
   */
  notifyScreenUpdated(): void {
    if (this.isStopped) return; // markExited も stop を通る
    this.scheduleProfile();
  }

  notifyInput(): void {
    this.bytes.notifyInput(this.opts.now());
  }

  notifyResize(): void {
    this.bytes.notifyResize(this.opts.now());
  }

  setTitle(title: string): void {
    if (this.isExited) return;
    this.title.setTitle(title, this.opts.now());
    this.evaluate();
  }

  /** 子プロセスの終了。終端の状態で、以後は何も判定しない。 */
  markExited(): void {
    this.stop();
    this.isExited = true;
    this.decide('exited', 'exit');
  }

  /** タイマーを止める (dispose 用)。状態は変えない。 */
  stop(): void {
    this.isStopped = true;
    clearInterval(this.tickTimer);
    if (this.profileTimer) clearTimeout(this.profileTimer);
    this.profileTimer = undefined;
    this.profileWaitSince = undefined;
  }

  private tick(): void {
    this.byteVerdict = this.bytes.tick(this.opts.now());
    this.evaluate();
  }

  private scheduleProfile(): void {
    const { profile } = this;
    if (!profile) return;
    const now = this.opts.now();
    this.profileWaitSince ??= now;
    const delay = Math.max(0, Math.min(PROFILE_DEBOUNCE_MS, this.profileWaitSince + PROFILE_MAX_WAIT_MS - now));
    if (this.profileTimer) clearTimeout(this.profileTimer);
    this.profileTimer = setTimeout(() => this.runProfile(profile), delay);
    this.profileTimer.unref();
  }

  private runProfile(profile: AgentProfile): void {
    this.profileTimer = undefined;
    this.profileWaitSince = undefined;
    this.profileVerdict = this.classify(profile, this.opts.readScreen());
    this.evaluate();
  }

  /**
   * classify は外から差し込まれる実装なので、ここを境界として例外を受け止める。
   * 失敗は「意見なし」(title / bytes 段に任せる) として扱い、続けて失敗したらこの pane では無効にする。
   */
  private classify(profile: AgentProfile, screen: ProfileScreen): ProfileVerdict {
    try {
      const verdict = profile.classify(screen);
      this.profileFailures = 0;
      return verdict;
    } catch (e) {
      this.profileFailures++;
      this.opts.log.warn(`profile ${profile.name} failed (${this.profileFailures}/${PROFILE_MAX_FAILURES}): ${(e as Error).message}`);
      if (this.profileFailures >= PROFILE_MAX_FAILURES) {
        this.profile = undefined;
        this.opts.log.warn(`profile ${profile.name} disabled after ${PROFILE_MAX_FAILURES} consecutive failures`);
      }
      return null;
    }
  }

  /** 各段の意見を優先順に見て、最初に意見を持った段で決める。全段が意見なしなら現状維持。 */
  private evaluate(): void {
    if (this.isExited) return;
    const { profile } = this;
    if (profile && this.profileVerdict !== null) return this.decide(this.profileVerdict, profile.name);
    const titleVerdict = this.title.verdict(this.opts.now());
    if (titleVerdict !== null) return this.decide(titleVerdict, 'title');
    if (this.byteVerdict !== null) this.decide(this.byteVerdict, 'bytes');
  }

  private decide(state: AgentState, decidedBy: string): void {
    if (state === this.state && decidedBy === this.decidedBy) return;
    const prev = this.state;
    this.state = state;
    this.decidedBy = decidedBy;
    this.opts.onChange(state, decidedBy, prev);
  }
}
